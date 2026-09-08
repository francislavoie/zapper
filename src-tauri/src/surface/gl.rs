//! GL entry points resolved by dlopen.
//!
//! Deliberately loads `libGL.so.1` rather than libepoxy. libepoxy exports
//! `epoxy_glClearColor` as a *data* symbol - a function pointer - and the
//! unprefixed names exist only as header macros. `dlsym(libepoxy,
//! "glClearColor")` therefore finds nothing, every call becomes a no-op, and
//! the GLArea renders an empty surface while still reporting a healthy render
//! callback count and no error. `resolves()` exists so that failure is loud.

use std::ffi::{c_void, CStr, CString};
use std::sync::OnceLock;

pub const COLOR_BUFFER_BIT: u32 = 0x0000_4000;
/// `GL_DRAW_FRAMEBUFFER_BINDING`, which shares its value with
/// `GL_FRAMEBUFFER_BINDING`. GtkGLArea binds its own FBO for the duration of
/// the render signal, so anything drawing into framebuffer 0 there paints
/// somewhere GTK never composites.
pub const DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
pub const FRAMEBUFFER: u32 = 0x8D40;
pub const RGBA: u32 = 0x1908;
pub const UNSIGNED_BYTE: u32 = 0x1401;
pub const VENDOR: u32 = 0x1F00;
pub const RENDERER: u32 = 0x1F01;
pub const VERSION: u32 = 0x1F02;

static LIB: OnceLock<Option<libloading::Library>> = OnceLock::new();

pub fn library() -> Option<&'static libloading::Library> {
    LIB.get_or_init(|| unsafe {
        libloading::Library::new("libGL.so.1")
            .or_else(|_| libloading::Library::new("libGL.so"))
            .map_err(|e| tracing::error!("no GL library: {e}"))
            .ok()
    })
    .as_ref()
}

/// True if `name` (nul-terminated) is a real exported symbol.
///
/// libloading reinterprets the `dlsym` address as the requested type rather
/// than reading through it, so asking for a `*const c_void` yields the
/// symbol's own address - exactly the "does this exist" question.
///
/// # Safety
/// `name` must be nul-terminated.
pub unsafe fn resolves(name: &[u8]) -> bool {
    library()
        .and_then(|l| l.get::<*const c_void>(name).ok())
        .is_some()
}

/// # Safety
/// `T` must match the real signature of the symbol named by `name`, which must
/// be nul-terminated.
unsafe fn sym<T: Copy>(name: &[u8]) -> Option<T> {
    library()?.get::<T>(name).ok().map(|s| *s)
}

/// Address lookup for libmpv's render API, which resolves GL itself.
///
/// mpv asks for far more than the handful of entry points below - framebuffer
/// and shader calls among them - so this hands it raw `dlsym` results. A name
/// libGL does not export yields null, which mpv treats as "extension absent".
pub fn get_proc_address(name: &str) -> *mut c_void {
    let Ok(c) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    // `c` owns the buffer for the whole call, so the borrow handed to `get`
    // cannot dangle.
    let bytes = c.as_bytes_with_nul();
    unsafe {
        library()
            .and_then(|l| l.get::<*mut c_void>(bytes).ok())
            .map(|s| *s)
            .unwrap_or(std::ptr::null_mut())
    }
}

/// # Safety
/// Requires a current GL context.
pub unsafe fn clear_color(r: f32, g: f32, b: f32, a: f32) {
    if let Some(f) = sym::<extern "C" fn(f32, f32, f32, f32)>(b"glClearColor\0") {
        f(r, g, b, a);
    }
}

/// # Safety
/// Requires a current GL context.
pub unsafe fn clear(mask: u32) {
    if let Some(f) = sym::<extern "C" fn(u32)>(b"glClear\0") {
        f(mask);
    }
}

/// # Safety
/// Requires a current GL context; `fbo` must name a live framebuffer or 0.
pub unsafe fn bind_framebuffer(target: u32, fbo: i32) {
    if let Some(f) = sym::<extern "C" fn(u32, u32)>(b"glBindFramebuffer\0") {
        f(target, fbo as u32);
    }
}

/// Reads a block of pixels from the bound read framebuffer as RGBA bytes.
///
/// This is how "is the surface actually black?" gets answered without a
/// screenshot: GNOME refuses D-Bus captures and `gtk_widget_draw` does not
/// pull GL content into Cairo, so the only honest look at what GL produced is
/// to read it back from the framebuffer GTK is about to composite.
///
/// # Safety
/// Requires a current GL context; `buf` must hold at least `w * h * 4` bytes.
pub unsafe fn read_pixels_rgba(x: i32, y: i32, w: i32, h: i32, buf: &mut [u8]) -> bool {
    debug_assert!(buf.len() >= (w as usize) * (h as usize) * 4);
    let Some(f) = sym::<extern "C" fn(i32, i32, i32, i32, u32, u32, *mut u8)>(b"glReadPixels\0")
    else {
        return false;
    };
    f(x, y, w, h, RGBA, UNSIGNED_BYTE, buf.as_mut_ptr());
    true
}

/// # Safety
/// Requires a current GL context.
pub unsafe fn get_error() -> u32 {
    sym::<extern "C" fn() -> u32>(b"glGetError\0").map_or(0xFFFF_FFFF, |f| f())
}

/// # Safety
/// Requires a current GL context; `pname` must name a single-integer state.
pub unsafe fn get_integer(pname: u32) -> i32 {
    let mut v: i32 = 0;
    if let Some(f) = sym::<extern "C" fn(u32, *mut i32)>(b"glGetIntegerv\0") {
        f(pname, &mut v as *mut i32);
    }
    v
}

/// # Safety
/// Requires a current GL context, and `dst` must have room for
/// `width * height * 4` bytes.
#[allow(clippy::too_many_arguments)]
pub unsafe fn read_pixels(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    format: u32,
    ty: u32,
    dst: *mut u8,
) {
    if let Some(f) =
        sym::<extern "C" fn(i32, i32, i32, i32, u32, u32, *mut u8)>(b"glReadPixels\0")
    {
        f(x, y, width, height, format, ty, dst);
    }
}

/// # Safety
/// Requires a current GL context; `name` must be a valid `glGetString` enum.
pub unsafe fn get_string(name: u32) -> Option<String> {
    let f = sym::<extern "C" fn(u32) -> *const i8>(b"glGetString\0")?;
    let p = f(name);
    if p.is_null() {
        return None;
    }
    Some(CStr::from_ptr(p).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gl_library_loads() {
        assert!(
            library().is_some(),
            "libGL.so.1 not loadable; install libgl1"
        );
    }

    #[test]
    fn required_symbols_resolve() {
        // The whole point: libepoxy would pass library() and fail this.
        for sym in [&b"glClearColor\0"[..], b"glClear\0", b"glGetString\0"] {
            assert!(
                unsafe { resolves(sym) },
                "{} did not resolve; GL calls would silently no-op",
                String::from_utf8_lossy(sym),
            );
        }
    }
}
