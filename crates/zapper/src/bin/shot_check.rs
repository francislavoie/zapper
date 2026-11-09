//! Reconstructs a picture of the running window, for verification with no eyes.
//!
//! Usage: `ZAPPER_STREAM_URL=<url> cargo run --bin shot_check -- [out.png] [seconds]`
//!
//! GNOME will not screenshot for an unauthorised caller, Wayland-native
//! windows capture black, and XWayland has no shared framebuffer to grab. So
//! this composites the window from its two layers instead: the video read back
//! out of the GL framebuffer, and the interface rendered through Cairo with
//! its alpha intact.
//!
//! It is a reconstruction, not a screenshot - it cannot prove the compositor
//! puts the result on screen. What it does prove is everything that has
//! actually gone wrong so far: a black surface, an inverted picture, chrome
//! painting opaque over video, a video rect in the wrong place.
//!
//! Audio is off outright (`ao=null`): this runs unattended.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("shot_check only runs on Linux/GTK");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    use gtk::prelude::GLAreaExt;
    use tauri::Manager;
    use zapper_lib::player::PlayerBackend;

    tracing_subscriber::fmt::init();

    // The fixture server's sample unless told otherwise; a URL off this
    // machine only with ZAPPER_ALLOW_REAL_ACCOUNT=1 (see zapper_lib::harness).
    let url = match zapper_lib::harness::stream_url(
        "shot_check",
        std::env::var("ZAPPER_STREAM_URL").ok(),
    ) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(2);
        }
    };
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/zapper-shot.png".into());
    let seconds: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(12);

    tauri::Builder::default()
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            let ov = zapper_lib::surface::linux::overlay::install(&window)?;
            let player = zapper_lib::surface::linux::render::new_player(&ov.gl_area)?;
            player.set_property("ao", "null")?;
            player.load(&url)?;

            let tick = ov.gl_area.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                tick.queue_render();
                gtk::glib::ControlFlow::Continue
            });

            // Dock the video partway, so the capture shows whether the rect is
            // honoured rather than only that something rendered.
            let dock = ov.clone();
            gtk::glib::timeout_add_seconds_local_once(seconds.saturating_sub(4).max(1), move || {
                dock.set_video_rect(zapper_lib::surface::VideoRect {
                    x: 0.55,
                    y: 0.55,
                    w: 0.42,
                    h: 0.40,
                });
            });

            let shot = ov.clone();
            let out2 = out.clone();
            gtk::glib::timeout_add_seconds_local_once(seconds, move || {
                zapper_lib::surface::linux::capture::request(&out2);
                // The grab happens inside the next render; give it frames, then
                // composite and write.
                let finish = shot.clone();
                let path = out2.clone();
                gtk::glib::timeout_add_seconds_local_once(2, move || {
                    match zapper_lib::surface::linux::capture::finish(&finish) {
                        Ok(p) => {
                            println!(
                                "PASS: wrote {} ({}x{}, peak sample {})",
                                p.display(),
                                gtk::prelude::WidgetExt::allocated_width(&finish.overlay),
                                gtk::prelude::WidgetExt::allocated_height(&finish.overlay),
                                finish.peak_sample(),
                            );
                            std::process::exit(0);
                        }
                        Err(e) => {
                            eprintln!("FAIL: {e} (wanted {path})");
                            std::process::exit(1);
                        }
                    }
                });
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("shot_check failed to run");
}
