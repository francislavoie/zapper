use std::path::{Path, PathBuf};

// `bundle()`, `without_bundle()`, `musl()`: shared with zapper-server's build script.
include!("../../packaging/libmpv/bundle.rs");

fn main() {
    println!("cargo:rustc-check-cfg=cfg(zapper_no_thumbframe)");
    let bundle = bundle();
    libmpv(bundle.as_deref());
    // Windows (MSVC) only.
    static_vcruntime::metabuild();
}

/// libmpv: libmpv2-sys asks for `mpv`, which [`bundle`] has put on the link path.
///
/// And the seek bar's previews of a title (`src/thumbframe.c`, one picture decoded from an
/// excerpt of mpv's buffer), compiled against the FFmpeg that libmpv already loads: the
/// bundle's libavformat, libavcodec, libswscale and libavutil, its headers in `include/`, or
/// with none fetched whatever pkg-config finds. Nothing is added to what ships.
///
/// With none fetched and pkg-config unable to find them (a Mac without FFmpeg's development
/// files, say), it is built without that file, with a warning: `zapper_no_thumbframe`
/// set, a title's seek bar previews only the frames drawn while it played.
fn libmpv(bundle: Option<&Path>) {
    let src = "src/thumbframe.c";
    println!("cargo:rerun-if-changed={src}");
    let mut build = cc::Build::new();
    build.file(src).warnings(true).flag_if_supported("-std=c11");
    let libs = ["avformat", "avcodec", "swscale", "avutil"];
    match bundle {
        Some(dir) => {
            build.include(dir.join("include"));
            build.compile("zapper_thumbframe");
            for lib in libs {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
        None => {
            without_bundle("zapper's libmpv");
            println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
            for lib in libs {
                match pkg_config::Config::new().probe(&format!("lib{lib}")) {
                    Ok(found) => {
                        for dir in found.include_paths {
                            build.include(dir);
                        }
                    }
                    Err(e) => {
                        println!(
                            "cargo:warning=lib{lib}'s development files aren't found ({e}); \
                             the seek bar of a movie or an episode will preview only what has played"
                        );
                        println!("cargo:rustc-cfg=zapper_no_thumbframe");
                        return;
                    }
                }
            }
            build.compile("zapper_thumbframe");
        }
    }
}
