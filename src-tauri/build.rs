use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_libmpv();
    }
    tauri_build::build();
}

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
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib").join("windows");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rerun-if-changed={}", dir.join("mpv.lib").display());
    println!("cargo:rerun-if-changed={}", dir.join("libmpv-2.dll").display());

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
