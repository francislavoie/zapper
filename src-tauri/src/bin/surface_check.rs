//! Verifies the playback surface against a real stream.
//!
//! Usage: `ZAPPER_STREAM_URL=<url> cargo run --bin surface_check -- [seconds]`
//!
//! The URL defaults to the fixture server's `sample1080.ts`, and one off this
//! machine is refused unless `ZAPPER_ALLOW_REAL_ACCOUNT=1` (see
//! `zapper_lib::harness`). It arrives by environment variable on purpose: an
//! Xtream live URL embeds the account username and password, and a command
//! line is world readable through `/proc` and lands verbatim in crash dumps.
//!
//! Audio is disabled outright (`ao=null`), not muted: this runs unattended and
//! must never make noise.
//!
//! Exits 0 only if GL resolved, the GLArea rendered repeatedly, mpv actually
//! painted those frames, and mpv reported real video dimensions. Anything else
//! exits 1 with the reason. This exists because GTK/GL/mpv wiring cannot be
//! unit tested: every failure mode in this stack is silent, so the assertions
//! have to run against a live window and a live stream.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("surface_check only runs on Linux/GTK");
    std::process::exit(2);
}

/// Where the harness parks the picture partway through.
///
/// Top-right, which is where the interface docks it while browsing. The
/// window behind this harness loads the real frontend, but the harness is
/// not driving it: it talks to the surface and the player directly, and the
/// IPC commands the interface would use are not even registered here. So the
/// app on screen is inert scenery, showing "nothing playing" while a stream
/// plays over it. Matching its corner at least stops the two contradicting
/// each other while somebody is watching the test run.
#[cfg(target_os = "linux")]
const DOCKED: zapper_lib::surface::VideoRect = zapper_lib::surface::VideoRect {
    x: 0.6,
    y: 0.05,
    w: 0.38,
    h: 0.32,
};

#[cfg(target_os = "linux")]
fn main() {
    use gtk::prelude::GLAreaExt;
    use tauri::Manager;

    tracing_subscriber::fmt::init();

    let url = match zapper_lib::harness::stream_url(
        "surface_check",
        std::env::var("ZAPPER_STREAM_URL").ok(),
    ) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(2);
        }
    };
    // How long to give the stream before judging it. Live HLS/TS over a slow
    // link needs more than a couple of seconds to produce a first frame.
    let seconds: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    tauri::Builder::default()
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            let ov = zapper_lib::surface::linux::overlay::install(&window)?;
            let player = zapper_lib::surface::linux::render::new_player(&ov.gl_area)?;
            // Kill the audio path entirely before anything is loaded. Muting or
            // zeroing the volume still opens the device and still leaks sound
            // on any code path that resets it; `ao=null` cannot.
            player.set_property("ao", "null")?;
            player.load(&url)?;

            // Nothing drives the GLArea until mpv's update callback fires, and
            // if mpv never decodes, that never happens. Ticking keeps the
            // render count honest about whether the *surface* works, which is
            // a separate failure from whether the *stream* works.
            let tick = ov.gl_area.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                tick.queue_render();
                gtk::glib::ControlFlow::Continue
            });

            // Log decode progress separately from render health, so a black
            // window can be attributed: real dimensions plus a black surface is
            // a render-target bug, no dimensions is a stream that never decoded.
            let probe = ov.clone();
            let probed = std::rc::Rc::new(std::cell::Cell::new(0u32));
            gtk::glib::timeout_add_seconds_local(1, move || {
                let n = probed.get() + 1;
                probed.set(n);
                tracing::info!(
                    "t+{n}s: renders={} mpv_painted={} peak_sample={}",
                    probe.render_count(),
                    probe.mpv_draw_count(),
                    probe.peak_sample(),
                );
                gtk::glib::ControlFlow::Continue
            });

            // Docking end to end: dock the video partway through, so the
            // verdict can prove the rect actually moved the GL surface rather
            // than only passing its unit tests.
            let dock = ov.clone();
            gtk::glib::timeout_add_seconds_local_once(seconds / 2, move || {
                dock.set_video_rect(DOCKED);
            });

            gtk::glib::timeout_add_seconds_local(seconds, move || match verdict(&ov, &player) {
                Ok(msg) => {
                    println!("PASS: {msg}");
                    std::process::exit(0)
                }
                Err(e) => {
                    eprintln!("FAIL: {e}");
                    std::process::exit(1)
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("surface_check failed to run");
}

#[cfg(target_os = "linux")]
fn verdict(
    ov: &zapper_lib::surface::linux::overlay::Overlay,
    player: &zapper_lib::surface::mpv::Player,
) -> anyhow::Result<String> {
    use anyhow::anyhow;
    use gtk::prelude::{GLAreaExt, WidgetExt};
    use zapper_lib::surface::linux::gl;

    let renders = ov.render_count();
    if renders < 30 {
        return Err(anyhow!(
            "only {renders} render callbacks; surface is not drawing"
        ));
    }
    if let Some(e) = ov.gl_area.error() {
        return Err(anyhow!("GLArea reported an error: {e}"));
    }
    let drawn = ov.mpv_draw_count();
    if drawn < 30 {
        return Err(anyhow!(
            "mpv painted only {drawn} of {renders} frames; the surface is clearing to black"
        ));
    }
    let (w, h) = player
        .dimensions()
        .ok_or_else(|| anyhow!("mpv reported no video dimensions; stream never decoded"))?;
    if w <= 0 || h <= 0 {
        return Err(anyhow!("mpv reported {w}x{h}"));
    }

    // The one check that a healthy render count cannot fake. Reading into the
    // wrong framebuffer - GTK binds its own during the render signal, so
    // framebuffer 0 is never composited - produces a black window that passes
    // every other assertion here.
    let peak = ov.peak_sample();
    if peak == 0 {
        return Err(anyhow!(
            "surface is black: mpv decoded {w}x{h} and painted {drawn} frames, but the \
             framebuffer's centre never exceeded 0. Wrong render target, not a decode failure."
        ));
    }
    // The docked rect must have actually resized the GL surface. A rect that
    // silently does nothing looks identical to one that works.
    let (ow, oh) = (ov.overlay.allocated_width(), ov.overlay.allocated_height());
    let (aw, ah) = (ov.gl_area.allocated_width(), ov.gl_area.allocated_height());
    if ow <= 1 || oh <= 1 {
        return Err(anyhow!(
            "overlay has no allocation ({ow}x{oh}); nothing is laid out"
        ));
    }
    if aw >= ow || ah >= oh {
        return Err(anyhow!(
            "set_video_rect did not dock the surface: GLArea {aw}x{ah} in overlay {ow}x{oh}"
        ));
    }

    // The surface must never pin the window's minimum size.
    //
    // Otherwise the app becomes unresizable. Sizing the surface with
    // `set_size_request` asks GTK for a *minimum*, and a minimum on a widget
    // inside the toplevel is a minimum on the toplevel: going fullscreen
    // would ask for a surface the size of the display, so the window could
    // never again be made smaller than the display. It latches, because a
    // window that cannot shrink goes on measuring full-screen and asking for
    // a full-screen surface.
    //
    // Asking for the whole window is the case that triggers it, so that is
    // what is asked for here.
    ov.set_video_rect(zapper_lib::surface::VideoRect::full());
    let (req_w, req_h) = ov.gl_area.size_request();
    if req_w > 0 || req_h > 0 {
        return Err(anyhow!(
            "the GL surface has a size request of {req_w}x{req_h}; that is a minimum, and it              propagates to the toplevel, which is what stopped the window being resized"
        ));
    }
    let (min_w, _) = ov.overlay.preferred_width();
    let (min_h, _) = ov.overlay.preferred_height();
    if min_w >= ow || min_h >= oh {
        return Err(anyhow!(
            "a full-window video rect made the overlay demand at least {min_w}x{min_h} inside              an allocation of {ow}x{oh}; the window would be stuck at that size"
        ));
    }

    // Scrollback thumbnails come off the same framebuffer. Every failure in
    // that path is silent - a missing blit entry point, an incomplete
    // offscreen framebuffer, a readback that returns nothing - and all of
    // them look exactly like "the user never hovered the scrubber". The only
    // way to tell is to count what was actually captured.
    let thumbs = zapper_lib::surface::trickplay::count();
    if thumbs == 0 {
        return Err(anyhow!(
            "no scrollback thumbnails after {renders} frames; the downscale blit or the \
             readback is failing silently"
        ));
    }
    // And they have to be findable, not merely stored: `nearest_png` is what
    // the command calls, and an empty PNG would still count above.
    let probe = player.probe();
    let png = probe
        .time_pos
        .and_then(zapper_lib::surface::trickplay::nearest_png)
        .map(|(_, bytes)| bytes.len())
        .unwrap_or(0);
    if png < 100 {
        return Err(anyhow!(
            "{thumbs} thumbnails captured but the one at the playhead encoded to {png} bytes"
        ));
    }

    // Put back what the probe borrowed. Nothing is asserted after this, but
    // the harness stays on screen until the process exits and a window that
    // ends showing full-screen video over a browsing interface is a window
    // that looks like the bug this check exists to catch.
    ov.set_video_rect(DOCKED);

    // glGetString needs a *current* context, and nothing is current on a
    // plain timeout tick outside the render signal.
    ov.gl_area.make_current();
    let renderer = unsafe { gl::get_string(gl::RENDERER) }
        .ok_or_else(|| anyhow!("glGetString returned null; no live GL context"))?;
    Ok(format!(
        "{renders} renders ({drawn} painted by mpv), video {w}x{h}, peak sample {peak}, \
         docked GLArea {aw}x{ah} in overlay {ow}x{oh}, window minimum {min_w}x{min_h} with a \
         full-window rect, {thumbs} scrollback thumbnails ({png}-byte PNG), GL renderer \
         {renderer}"
    ))
}
