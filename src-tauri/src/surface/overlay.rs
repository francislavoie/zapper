//! GTK widget surgery: put a GL surface beneath Tauri's webview.
//!
//! The webview is removed from its container, a GtkOverlay holding a GLArea
//! takes its place, and the webview is re-added as the overlay child so it
//! paints on top with a transparent background. Measured: CSS alpha 0.55
//! arrives as 140/255, so per-pixel blending is intact.
//!
//! Exactly ONE `connect_render` handler exists in the whole program, and it
//! lives here. GTK3's `render` signal is `RUN_LAST` with a boolean
//! accumulator, so a handler returning `Propagation::Stop` halts emission and
//! any later-connected handler never runs. mpv therefore does not connect its
//! own; it exposes [`crate::surface::mpv::draw`], which this handler calls.

use anyhow::{anyhow, Result};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use webkit2gtk::WebViewExt;

use crate::surface::gl;

thread_local! {
    /// `with_webview` demands a `Send` closure while every GTK object is
    /// `!Send`, so results come back through here. Safe because Tauri runs
    /// that closure inline on the GTK main thread.
    static BUILT: RefCell<Option<Result<Overlay, String>>> = const { RefCell::new(None) };
}

/// The composited stack: a GL surface with Tauri's webview floating over it.
#[derive(Clone)]
pub struct Overlay {
    pub overlay: gtk::Overlay,
    pub gl_area: gtk::GLArea,
    /// Where the surface goes, normalised. Read back by the overlay's
    /// `get-child-position` handler, which is the only thing that positions
    /// the GL surface - see [`Overlay::set_video_rect`].
    placed: Rc<Cell<crate::surface::VideoRect>>,
    renders: Rc<Cell<u32>>,
    mpv_draws: Rc<Cell<u32>>,
    peak: Rc<Cell<u8>>,
}

impl Overlay {
    /// How many times the GLArea has actually drawn. A surface that is laid
    /// out but never rendering reports zero here while looking healthy
    /// everywhere else, so the verification harness asserts on it.
    pub fn render_count(&self) -> u32 {
        self.renders.get()
    }

    /// How many of those renders mpv actually painted. A surface that clears
    /// to black every frame has a healthy `render_count` and zero here, which
    /// is exactly the silent failure this stack is prone to.
    pub fn mpv_draw_count(&self) -> u32 {
        self.mpv_draws.get()
    }

    /// The brightest channel value seen so far in the centre of the surface.
    ///
    /// Zero after seconds of live video means the surface is black - which is
    /// what rendering into framebuffer 0 instead of GtkGLArea's own FBO looks
    /// like, and it is otherwise indistinguishable from success: healthy
    /// render count, mpv reporting frames painted, no GL error.
    pub fn peak_sample(&self) -> u8 {
        self.peak.get()
    }

    /// Positions the GL surface. Called on browse/fullscreen transitions.
    ///
    /// Records the rect and asks for a re-layout; the overlay's
    /// `get-child-position` handler turns it into an allocation. It is done
    /// that way round, rather than with a size request, because a size
    /// request is a *minimum* and GTK propagates it: the surface is inside
    /// the toplevel, so asking for a surface the size of the screen would ask
    /// for a window that could never again be smaller than the screen.
    ///
    /// And it would latch. Going fullscreen would set the minimum to the whole
    /// display; leaving fullscreen could not shrink the window past that
    /// minimum, so the stage would never measure smaller, so the next rect
    /// would be full-screen again: a window stuck at the size of the monitor,
    /// refusing to be resized, with nothing in the logs.
    pub fn set_video_rect(&self, rect: crate::surface::VideoRect) {
        self.placed.set(rect);
        self.overlay.queue_resize();
    }
}

/// Reads back a small block from the middle of the surface and records the
/// brightest channel seen. Called from inside the render signal, where the
/// GLArea's own framebuffer is bound.
fn sample_centre(area: &gtk::GLArea, peak: &Rc<Cell<u8>>) {
    const N: i32 = 32;
    let scale = area.scale_factor();
    let w = area.allocated_width() * scale;
    let h = area.allocated_height() * scale;
    if w < N || h < N {
        return;
    }
    let mut buf = [0u8; (N * N * 4) as usize];
    let read = unsafe { gl::read_pixels_rgba((w - N) / 2, (h - N) / 2, N, N, &mut buf) };
    if !read {
        return;
    }
    // Alpha is skipped: an opaque black surface has alpha 255 and would make
    // a dead framebuffer look bright.
    let brightest = buf
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| px[..3].iter().copied())
        .max()
        .unwrap_or(0);
    if brightest > peak.get() {
        peak.set(brightest);
    }
}

/// Reparents `window`'s webview above a fresh GLArea and returns the stack.
///
/// Must be called on the GTK main thread, which is where Tauri's `setup` hook
/// runs.
pub fn install(window: &tauri::WebviewWindow) -> Result<Overlay> {
    // Refuse to start with no-op GL rather than render an invisible surface.
    for s in [&b"glClearColor\0"[..], b"glClear\0"] {
        if !unsafe { gl::resolves(s) } {
            return Err(anyhow!(
                "GL symbol {} did not resolve; refusing to start with no-op GL",
                String::from_utf8_lossy(s)
            ));
        }
    }

    window.with_webview(|platform| {
        let webview = platform.inner();
        // The *webview* is transparent; the window is not, and does not need
        // to be. This alpha is what lets the GL surface show through the page
        // — compositing inside the window, which has nothing to do with how
        // the window meets the desktop. Asking for a transparent toplevel as
        // well only bought an RGBA visual for the compositor to blend against
        // the wallpaper on every frame, for a window that is opaque from edge
        // to edge.
        webview.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));

        suppress_browser_menu(&webview);
        tune_scrolling(&webview);

        let Some(parent) = webview.parent() else {
            tracing::error!("webview has no parent container; cannot install overlay");
            return;
        };
        let Ok(container) = parent.downcast::<gtk::Container>() else {
            tracing::error!("webview parent is not a GtkContainer; cannot install overlay");
            return;
        };

        let gl_area = gtk::GLArea::new();
        // No expand or align flags: this is an overlay child whose allocation
        // is supplied outright by `get-child-position` below, and a flag that
        // looks like it positions the surface but does not would be worse
        // than none at all.
        gl_area.set_has_alpha(false);

        let renders = Rc::new(Cell::new(0u32));
        let mpv_draws = Rc::new(Cell::new(0u32));
        let peak = Rc::new(Cell::new(0u8));
        let counter = renders.clone();
        let drawn = mpv_draws.clone();
        let peaked = peak.clone();
        gl_area.connect_render(move |area, _| {
            let n = counter.get() + 1;
            counter.set(n);
            // The ONLY render handler. mpv draws through this hook once it
            // exists; until then we clear to black.
            let painted = crate::surface::mpv::draw(area);
            // Read back here rather than inside mpv's draw: this handler runs
            // every frame, so a capture still works when nothing is playing
            // and the surface is only being cleared.
            {
                let scale = area.scale_factor();
                crate::surface::capture::maybe_grab(
                    area.allocated_width() * scale,
                    area.allocated_height() * scale,
                );
            }
            if painted {
                drawn.set(drawn.get() + 1);
                // A copy of the frame that was just drawn, for the scrubber.
                // Nothing is decoded for this: it is already on the GPU, and
                // it was about to be discarded. The gate is checked first so
                // that the ordinary frame costs one comparison and does not
                // read mpv's clock sixty times a second.
                if crate::surface::trickplay::is_due() {
                    if let Some(Some(at)) =
                        crate::surface::mpv::with_player(|p| p.get_f64("playback-time"))
                    {
                        let scale = area.scale_factor();
                        unsafe {
                            crate::surface::trickplay::capture(
                                area.allocated_width() * scale,
                                area.allocated_height() * scale,
                                at,
                            );
                        }
                    }
                }
                // Twice a second, look at what actually landed in the
                // framebuffer. Cheap, and the only thing that can tell a
                // working surface from a black one.
                if n.is_multiple_of(30) {
                    sample_centre(area, &peaked);
                }
            } else {
                unsafe {
                    gl::clear_color(0.0, 0.0, 0.0, 1.0);
                    gl::clear(gl::COLOR_BUFFER_BIT);
                }
            }
            glib::Propagation::Proceed
        });

        let overlay = gtk::Overlay::new();
        container.remove(&webview);

        // The GL surface is an *overlay* child, never the overlay's main
        // child. The main child is what GTK asks for a minimum size, and the
        // surface has to be free to be any size at all without that reaching
        // the window. Added first, so it is the bottom-most overlay child and
        // the webview still floats above it.
        let placed = Rc::new(Cell::new(crate::surface::VideoRect::full()));
        overlay.add_overlay(&gl_area);
        // It is underneath the webview, which covers the window, so it would
        // never see an event anyway - but saying so is free and means input
        // can never be explained by this widget.
        overlay.set_overlay_pass_through(&gl_area, true);

        let positioned = gl_area.clone();
        let wanted = placed.clone();
        overlay.connect_get_child_position(move |overlay, widget| {
            // Only the GL surface is placed by hand. Returning None for
            // everything else leaves tao's box and the webview to the
            // overlay's own layout, which already fills them to the window.
            if widget != positioned.upcast_ref::<gtk::Widget>() {
                return None;
            }
            let (x, y, w, h) = wanted
                .get()
                .to_pixels(overlay.allocated_width(), overlay.allocated_height());
            Some(gdk::Rectangle::new(x, y, w, h))
        });

        // The overlay must become a *direct* child of the toplevel window, not
        // simply be dropped into tao's box. tauri-runtime-wry's
        // `undecorated_resizing` handler runs
        // `webview.parent().and_then(|w| w.parent()).downcast::<gtk::Window>().unwrap()`
        // on every button press and touch, so nesting the overlay inside that
        // box leaves the chain ending at a GtkBox and aborts the process on the
        // first click. Tao's box is not discarded - it is re-added as an
        // overlay child, so tauri's menu API still has somewhere to put a menu
        // bar - but the webview must be the overlay's own child for that
        // grandparent lookup to find the window.
        let window_parent = container
            .parent()
            .and_then(|w| w.downcast::<gtk::Window>().ok());

        match window_parent {
            Some(win) => {
                win.remove(&container);
                // Tao's box is the main child now that the GL surface is not.
                // It is empty - the webview has just been taken out of it - so
                // it asks for nothing, which is exactly what the toplevel's
                // minimum size should be built from. It is kept rather than
                // discarded because tauri's menu API still needs somewhere to
                // put a menu bar.
                overlay.add(&container);
                overlay.add_overlay(&webview);
                win.add(&overlay);
                escape_hatch(&win);
                survive_a_failed_interface(&webview, &win);
            }
            None => {
                // Unknown hierarchy: keep the old nesting rather than tear the
                // tree apart, but say so - clicks may abort in tauri's handler.
                tracing::warn!(
                    "webview's grandparent is not a GtkWindow; nesting the overlay instead. \
                     tauri's undecorated-resize handler may abort on click."
                );
                overlay.add_overlay(&webview);
                container.add(&overlay);
            }
        }
        paint_window_black();

        overlay.show_all();

        // tauri's resize handler unwraps this exact chain on every button press
        // and touch, so a broken tree is an abort waiting to happen rather than
        // a cosmetic problem. Assert it here, not when the user clicks.
        let chain_ok = webview
            .parent()
            .and_then(|w| w.parent())
            .is_some_and(|w| w.is::<gtk::Window>());

        BUILT.with(|b| {
            *b.borrow_mut() = Some(if chain_ok {
                Ok(Overlay {
                    overlay,
                    gl_area,
                    placed,
                    renders,
                    mpv_draws,
                    peak,
                })
            } else {
                Err(
                    "webview's grandparent is not a GtkWindow after reparenting; \
                     tauri's undecorated-resize handler would abort on the first click"
                        .to_owned(),
                )
            })
        });
    })?;

    BUILT
        .with(|b| b.borrow_mut().take())
        .ok_or_else(|| anyhow!("with_webview did not run; not a GTK platform?"))?
        .map_err(|e| anyhow!(e))
}

/// Paints the toplevel black, because the theme's colour is not black.
///
/// Everything in this stack is transparent by design: the page has no
/// background so that mpv shows through it, and the window is created
/// transparent. What is left underneath is GTK's own window background, which
/// takes its colour from the user's theme — white, on a light one.
///
/// That shows wherever the GL surface does not reach, which is exactly the
/// moments it is most visible: the instant before the first frame, and while
/// the window is growing into fullscreen and the surface has not yet been
/// told its new rect. It reads as a white box the picture grows into.
///
/// Applied to the screen rather than the widget: GTK3 draws the toplevel's
/// background from its style context, and a provider on the screen reaches it
/// without having to find the right widget in a tree this file has already
/// rearranged.
fn paint_window_black() {
    let Some(screen) = gdk::Screen::default() else {
        tracing::warn!("no GDK screen; the window keeps the theme's background colour");
        return;
    };
    let css = gtk::CssProvider::new();
    // `window` is the toplevel and `.background` is the style class GTK puts
    // on it. Both, because a theme may style either.
    if let Err(e) = css.load_from_data(b"window, window.background { background-color: #000; }") {
        tracing::warn!("could not parse the window background stylesheet: {e}");
        return;
    }
    gtk::StyleContext::add_provider_for_screen(
        &screen,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Turns off WebKit's own scroll animation.
///
/// `enable-smooth-scrolling` defaults on, and it makes WebKit animate every
/// discrete wheel tick over several frames rather than moving and stopping.
/// In a list that is the difference between scrolling and dragging: the
/// content keeps gliding after the wheel has stopped, which reads as lag
/// however fast the frames are.
///
/// It applies to *discrete* wheel events. A touchpad sends continuous scroll
/// deltas that the compositor handles, and those are unaffected, so this
/// buys a mouse wheel its native step without costing a touchpad its
/// kinetics.
fn tune_scrolling(webview: &webkit2gtk::WebView) {
    use webkit2gtk::{SettingsExt, WebViewExt};

    // Fully qualified: GtkWidget has a `settings()` of its own, and the one
    // wanted here is WebKit's.
    let Some(settings) = WebViewExt::settings(webview) else {
        tracing::warn!("no WebKit settings; wheel scrolling keeps its animation");
        return;
    };
    settings.set_enable_smooth_scrolling(false);
}

/// A way out that does not depend on the interface existing.
///
/// The window is undecorated and draws its own title bar in the webview, so
/// if the webview never loads there is no bar, no close button, and nothing
/// to click: the only way out is the taskbar. This binds Ctrl+Q at the GTK
/// level, below anything the page could be doing, so it works on a blank
/// window.
fn escape_hatch(win: &gtk::Window) {
    win.connect_key_press_event(|w, ev| {
        use gtk::gdk::keys::constants as key;
        let ctrl = ev
            .state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(ev.keyval(), key::q | key::Q) {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
}

/// Gives the user a way out when the interface fails to load.
///
/// The bar this app draws for itself lives in the page. If the page is not
/// there — a development build with no dev server, a corrupt install — then
/// neither is the bar, and an undecorated window with nothing in it cannot
/// be moved, resized or closed.
///
/// `set_decorated(true)` is the obvious answer and is not enough: GTK uses
/// client-side decorations on Wayland, so a window with no header bar
/// widget gains no buttons from it. The window stays exactly as unclosable
/// as it was. A header bar is attached instead, which is a real close
/// button, and a dialog says what happened — an error page nobody can
/// dismiss is only half an answer.
fn survive_a_failed_interface(webview: &webkit2gtk::WebView, win: &gtk::Window) {
    let window = win.clone();
    // Once: a failed load can be retried by WebKit, and three stacked
    // dialogs would be worse than the problem.
    let told = std::cell::Cell::new(false);
    webview.connect_load_failed(move |_, _event, uri, error| {
        // The URI is ours — a file path or the dev server — and carries no
        // credentials, unlike a stream URL.
        tracing::error!("the interface failed to load from {uri}: {error}");
        if told.replace(true) {
            return false;
        }
        // Not in a harness. The verification binaries load the same frontend
        // with no dev server behind it, so this fires every time they run —
        // a modal dialog and a header bar over whatever they were showing,
        // on somebody else's screen. They are not the app and nobody is
        // going to click Quit on them; the log line above is the whole
        // answer there.
        if std::env::var_os("ZAPPER_HARNESS").is_some() {
            return false;
        }

        // A real title bar, with a real close button in it.
        let bar = gtk::HeaderBar::new();
        bar.set_show_close_button(true);
        bar.set_title(Some("zapper"));
        bar.set_subtitle(Some("the interface could not be loaded"));
        window.set_titlebar(Some(&bar));
        window.set_decorated(true);
        bar.show_all();

        // And say so, because a blank window with a title bar is still a
        // blank window.
        let dialog = gtk::MessageDialog::new(
            Some(&window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Error,
            gtk::ButtonsType::None,
            "zapper could not load its interface.",
        );
        // Say which build this is, because the answer differs completely
        // and the binary's own path is the only reliable way to tell. A
        // release build carries its interface inside it and never reaches
        // for a server; only a development build does.
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "this binary".to_owned());
        let advice = if cfg!(debug_assertions) {
            "This is a development build. It loads the interface from the \
             Vite dev server, so `npm run app` has to be running — start it \
             from the project directory and launch again."
        } else {
            "This is a release build, which carries its interface inside it \
             and should never need a server. Seeing this means the build is \
             damaged; rebuild it with `npx tauri build`."
        };
        dialog.set_secondary_text(Some(&format!("{error}\n\n{advice}\n\nRunning: {exe}")));
        dialog.add_button("Quit", gtk::ResponseType::Close);
        let closing = window.clone();
        dialog.connect_response(move |d, _| {
            d.close();
            closing.close();
        });
        dialog.show_all();

        // False: let WebKit draw its error page behind all this too.
        false
    });
}

/// Stops WebKit offering Back / Forward / Stop / Reload / Inspect Element.
///
/// Those are a browser's menu, and this is not a browser: none of them mean
/// anything here, and "Reload" would throw away the running player. The one
/// case worth keeping is editing, where Cut / Copy / Paste are genuinely
/// useful, so a right-click in a text field gets a menu trimmed to exactly
/// those and nothing else.
///
/// This lives at the GTK level rather than as a `contextmenu` handler in the
/// page because a missed listener is a leaked browser menu, and the failure is
/// invisible until somebody right-clicks the one element nobody covered.
fn suppress_browser_menu(webview: &webkit2gtk::WebView) {
    use webkit2gtk::{ContextMenuAction, ContextMenuExt, ContextMenuItem, HitTestResultExt};

    webview.connect_context_menu(|_, menu, _, hit| {
        if !hit.context_is_editable() {
            // true = handled, show nothing. Real per-element menus belong here
            // later, built from the hit test.
            return true;
        }
        menu.remove_all();
        for action in [
            ContextMenuAction::Cut,
            ContextMenuAction::Copy,
            ContextMenuAction::Paste,
            ContextMenuAction::SelectAll,
        ] {
            menu.append(&ContextMenuItem::from_stock_action(action));
        }
        false
    });
}
