//! The macOS surface: not implemented.
//!
//! A slot, so that a macOS build compiles and fails at startup with a reason
//! rather than at link time with none. The likely shape, when there is a Mac
//! to build it on, is the Windows one: an `NSView` beneath the `WKWebView`
//! (made transparent with `drawsBackground = NO`), handed to mpv as `wid`
//! with `vo=gpu-next` and `gpu-api=vulkan` through MoltenVK, or mpv's render
//! API into a CAOpenGLLayer as on Linux.

use std::path::PathBuf;
use std::time::Duration;

use crate::surface::VideoRect;

const UNSUPPORTED: &str = "zapper cannot play video on macOS yet: there is no playback surface \
                           for it (src-tauri/src/surface/macos)";

pub(super) fn install(_window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(UNSUPPORTED))
}

pub(super) fn set_video_rect(_rect: VideoRect) -> Result<(), String> {
    Err(UNSUPPORTED.to_owned())
}

pub(super) fn repeat_on_main(_interval: Duration, _f: impl FnMut() -> bool + 'static) {
    tracing::error!("{UNSUPPORTED}");
}

pub(super) fn video_ready() -> bool {
    false
}

pub(super) fn request_frame() {}

pub(super) fn request_capture(_path: PathBuf, _after: Duration) {
    eprintln!("CAPTURE FAILED: {UNSUPPORTED}");
}
