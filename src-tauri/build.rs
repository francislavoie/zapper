#[cfg(feature = "gui")]
use std::path::{Path, PathBuf};

fn main() {
    #[cfg(feature = "gui")]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            windows_libmpv();
        }
        tauri_build::build();
    }
    #[cfg(feature = "server")]
    remuxer();
}

/// Compiles the recording service's remuxer (`src/recording/remux.c`)
/// against libavformat.
///
/// The FFmpeg libraries are found by pkg-config, so pointing the build at
/// another FFmpeg — a bundled LGPL build rather than the system's — is a
/// matter of `PKG_CONFIG_PATH`, with nothing here to change. The C side does
/// only what needs FFmpeg's structures (demuxing and muxing packets); every
/// decision about timing and naming is Rust's.
#[cfg(feature = "server")]
fn remuxer() {
    let src = "src/recording/remux.c";
    println!("cargo:rerun-if-changed={src}");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    let mut build = cc::Build::new();
    build.file(src).warnings(true).flag_if_supported("-std=c11");
    for lib in ["libavformat", "libavcodec", "libavutil"] {
        let found = pkg_config::Config::new()
            .probe(lib)
            .unwrap_or_else(|e| panic!("zapper-server needs {lib}'s development files: {e}"));
        for dir in found.include_paths {
            build.include(dir);
        }
    }
    build.compile("zapper_remux");
}

#[cfg(feature = "gui")]
/// Makes libmpv linkable and runnable on Windows.
///
/// libmpv2-sys asks the linker for `mpv`, which MSVC resolves to `mpv.lib`:
/// an import library that no Windows machine has. `tools/fetch-libmpv.ps1`
/// makes one, beside the DLL it describes, in `lib/windows/` (ignored by git;
/// see docs/windows.md). This points the linker there.
///
/// The DLL is also copied next to the executable cargo builds, because that
/// is the first place Windows looks for it, so `cargo run` and `tauri dev`
/// start without anyone editing PATH. Installers carry it the same way
/// (`bundle.resources` in tauri.windows.conf.json).
fn windows_libmpv() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("lib")
        .join("windows");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rerun-if-changed={}", dir.join("mpv.lib").display());
    println!(
        "cargo:rerun-if-changed={}",
        dir.join("libmpv-2.dll").display()
    );

    if !dir.join("mpv.lib").exists() {
        println!(
            "cargo:warning=libmpv is missing from {}; linking will fail. \
             Run tools/fetch-libmpv.ps1 (docs/windows.md).",
            dir.display()
        );
        return;
    }

    // OUT_DIR is target/<profile>/build/zapper-<hash>/out; the executable
    // goes in target/<profile>.
    let Some(profile_dir) = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| out.ancestors().nth(3).map(Path::to_path_buf))
    else {
        return;
    };
    let src = dir.join("libmpv-2.dll");
    let dst = profile_dir.join("libmpv-2.dll");
    let stale = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
        (Ok(s), Ok(d)) => s.len() != d.len() || s.modified().ok() > d.modified().ok(),
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if stale {
        if let Err(e) = std::fs::copy(&src, &dst) {
            println!("cargo:warning=could not copy libmpv-2.dll beside the executable: {e}");
        }
    }
}
