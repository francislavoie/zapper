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
    pub fn set_video_rect(&self, rect: crate::surface::VideoRect) {
        let (win_w, win_h) = (
            self.overlay.allocated_width(),
            self.overlay.allocated_height(),
        );
        let (x, y, w, h) = rect.to_pixels(win_w, win_h);
        self.gl_area.set_halign(gtk::Align::Start);
        self.gl_area.set_valign(gtk::Align::Start);
        self.gl_area.set_margin_start(x);
        self.gl_area.set_margin_top(y);
        self.gl_area.set_size_request(w, h);
        self.gl_area.queue_resize();
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
        webview.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));

        suppress_browser_menu(&webview);

        let Some(parent) = webview.parent() else {
            tracing::error!("webview has no parent container; cannot install overlay");
            return;
        };
        let Ok(container) = parent.downcast::<gtk::Container>() else {
            tracing::error!("webview parent is not a GtkContainer; cannot install overlay");
            return;
        };

        let gl_area = gtk::GLArea::new();
        gl_area.set_hexpand(true);
        gl_area.set_vexpand(true);
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
        overlay.add(&gl_area);

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
                overlay.add_overlay(&container);
                overlay.add_overlay(&webview);
                win.add(&overlay);
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
