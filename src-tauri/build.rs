use std::path::{Path, PathBuf};

fn main() {
    let bundle = bundle();
    #[cfg(feature = "gui")]
    {
        libmpv(bundle.as_deref());
        tauri_build::build();
    }
    #[cfg(feature = "server")]
    {
        remuxer(bundle.as_deref());
        web_assets();
        notices();
    }
    let _ = bundle;
    // Windows (MSVC) only; tauri-build does the same for the app.
    static_vcruntime::metabuild();
}

/// The web interface zapper-server serves (`recording::web`): the frontend's build in `../dist`,
/// the same files the app's window shows, compiled into the binary. One file to install and
/// nothing beside it to go missing or out of step with the API it talks to.
///
/// Writes `$OUT_DIR/web_assets.rs`, a list of `(path, include_bytes!(…))`; `include_bytes!`
/// makes a changed file rebuild, and watching `dist` notices a file added or removed. A release
/// build refuses without the frontend built (`npm run build`); a debug build serves a page
/// saying so, so the Rust tests and harnesses run without Node.
#[cfg(feature = "server")]
fn web_assets() {
    use std::fmt::Write as _;
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("dist"))
        .expect("src-tauri has a parent");
    // Made when missing, so there is a directory to watch: watching a path that does not exist
    // reruns this script, and rebuilds the crate, on every build. Vite empties and fills it.
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        collect(&dist, &dist, &mut files);
    } else if std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!("zapper-server serves the web interface from ../dist: run `npm run build` first");
    } else {
        println!("cargo:warning=../dist has no index.html: zapper-server's web interface will say it was not built (run `npm run build`)");
    }
    files.sort();
    let mut code = String::from("&[\n");
    for (rel, abs) in &files {
        let _ = writeln!(code, "    ({rel:?}, include_bytes!({abs:?})),");
    }
    code.push(']');
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("web_assets.rs");
    std::fs::write(&out, code).expect("writing web_assets.rs");
}

/// Every file under `dir`, as its path relative to `root` with `/` between the parts, and its
/// absolute path.
#[cfg(feature = "server")]
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            files.push((rel.join("/"), path.to_string_lossy().into_owned()));
        }
    }
}

/// zapper's own LGPL libmpv and FFmpeg: the archive `scripts/fetch-libmpv.sh` (or, on Windows,
/// `tools/fetch-libmpv.ps1`) unpacks into `lib/<platform>/`, made by `scripts/build-libmpv.sh`
/// (docs/licensing.md), or another directory of the same shape named by `ZAPPER_LIBMPV_DIR`.
/// The app links libmpv from it, and zapper-server's remuxer FFmpeg's libraries.
///
/// When it is there, its `link/` goes first on the link path (libmpv2-sys only asks for `mpv`,
/// the remuxer for `avformat` and the rest), and the executables are told where its `runtime/`
/// libraries will be when they run:
///
/// - Linux: `$ORIGIN/../lib/zapper`, where the .deb, .rpm and Arch package install them beside
///   `/usr/bin/zapper` and `/usr/bin/zapper-server`;
/// - macOS: `@executable_path/../Frameworks`, the app bundle's;
/// - Windows: beside the executable, where Windows looks first anyway.
///
/// So that `cargo run`, the harnesses and a release binary run from `target/` too, the runtime
/// libraries are copied to that same place relative to cargo's output, and a debug build also
/// records the fetched directory itself (for the test binaries, which live one level deeper).
/// A release build never records a path from the build machine.
///
/// Returns the directory, or None when nothing has been fetched (see [`without_bundle`]).
fn bundle() -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=ZAPPER_LIBMPV_DIR");
    println!("cargo:rerun-if-env-changed=ZAPPER_SYSTEM_LIBMPV");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // The static zapper-server for Linux links its own FFmpeg (`static_ffmpeg`), never the
    // shared bundle, which is glibc's.
    if musl() {
        return None;
    }
    let (platform, linkable, beside) = match (os.as_str(), arch.as_str()) {
        ("linux", "x86_64") => ("linux-x86_64", "libmpv.so", "lib/zapper"),
        // tools/fetch-libmpv.ps1 makes the import libraries MSVC links with.
        ("windows", "x86_64") => ("windows-x86_64", "mpv.lib", ""),
        ("macos", "aarch64") => ("macos-arm64", "libmpv.dylib", "Frameworks"),
        ("macos", "x86_64") => ("macos-x86_64", "libmpv.dylib", "Frameworks"),
        _ => return None,
    };
    let dir = std::env::var_os("ZAPPER_LIBMPV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib").join(platform));
    // Watched through its parent, which fetch-libmpv changes whenever it unpacks one: watching
    // a path that doesn't exist yet would rerun this script, and rebuild zapper, every time.
    if let Some(parent) = dir.parent() {
        let _ = std::fs::create_dir_all(parent);
        println!("cargo:rerun-if-changed={}", parent.display());
    }
    let link = dir.join("link");
    let runtime = dir.join("runtime");
    if !link.join(linkable).exists() {
        return None;
    }
    println!("cargo:rerun-if-changed={}", runtime.display());

    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    println!("cargo:rustc-link-search=native={}", link.display());
    match os.as_str() {
        "linux" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/zapper");
            if !release {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime.display());
            }
        }
        "macos" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
            if !release {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime.display());
            }
        }
        _ => {}
    }

    // OUT_DIR is <target>/[<triple>/]<profile>/build/zapper-<hash>/out: the executables are in
    // <profile>, and `beside` is relative to the directory above it.
    let profile_dir = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| out.ancestors().nth(3).map(Path::to_path_buf));
    let to = profile_dir.and_then(|p| {
        if beside.is_empty() {
            Some(p)
        } else {
            p.parent().map(|t| t.join(beside))
        }
    });
    if let (Some(to), Ok(entries)) = (to, std::fs::read_dir(&runtime)) {
        for e in entries.flatten() {
            copy_if_stale(&e.path(), &to.join(e.file_name()));
        }
    }
    Some(dir)
}

/// With nothing fetched, a debug build falls back to the system's libmpv or FFmpeg, with a
/// warning; a release build refuses, unless `ZAPPER_SYSTEM_LIBMPV=1` says linking the system's
/// is the point (a distribution's own package).
#[cfg(any(feature = "gui", feature = "server"))]
fn without_bundle(what: &str) {
    if std::env::var("ZAPPER_SYSTEM_LIBMPV").as_deref() == Ok("1") {
        return;
    }
    let msg = format!("{what} isn't fetched (run `just fetch-libmpv`, docs/licensing.md)");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if release || std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        panic!("{msg}; ZAPPER_SYSTEM_LIBMPV=1 links the system's instead");
    }
    println!("cargo:warning={msg}; linking the system's instead");
}

/// libmpv for the app: libmpv2-sys asks for `mpv`, which [`bundle`] has put on the link path.
#[cfg(feature = "gui")]
fn libmpv(bundle: Option<&Path>) {
    if bundle.is_none() {
        without_bundle("zapper's libmpv");
    }
}

/// Compiles the recording service's remuxers (`src/recording/remux.c`, a capture into Matroska,
/// `src/recording/livemux.c`, a live feed for a browser, and `src/recording/vodmux.c`, a movie file
/// for a browser) against FFmpeg's libavformat,
/// libavcodec, libswresample and libavutil: the bundle's (their headers in `include/`), or, with
/// none fetched, whatever pkg-config finds. The C side does only what needs FFmpeg's structures
/// (demuxing, decoding, encoding and muxing); every decision about timing and naming is Rust's.
#[cfg(feature = "server")]
fn remuxer(bundle: Option<&Path>) {
    let srcs = ["src/recording/remux.c", "src/recording/livemux.c", "src/recording/vodmux.c"];
    for src in srcs {
        println!("cargo:rerun-if-changed={src}");
    }
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    let mut build = cc::Build::new();
    build.files(srcs).warnings(true).flag_if_supported("-std=c11");
    if musl() {
        static_ffmpeg(&mut build);
        return;
    }
    let libs = ["avformat", "avcodec", "swresample", "avutil"];
    match bundle {
        Some(dir) => {
            build.include(dir.join("include"));
            build.compile("zapper_remux");
            // After the remuxer's archive, which needs them.
            for lib in libs {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
        None => {
            without_bundle("zapper's FFmpeg");
            for lib in libs {
                let found = pkg_config::Config::new()
                    .probe(&format!("lib{lib}"))
                    .unwrap_or_else(|e| panic!("zapper-server needs lib{lib}'s development files: {e}"));
                for dir in found.include_paths {
                    build.include(dir);
                }
            }
            build.compile("zapper_remux");
        }
    }
}

/// Building for musl: the static zapper-server for Linux, one file with nothing to load.
fn musl() -> bool {
    std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl")
}

/// The static zapper-server's FFmpeg: the static libraries `scripts/build-ffmpeg-static.sh` made
/// (only what the service uses, LGPL, docs/licensing.md), in the folder `ZAPPER_FFMPEG_STATIC`
/// names (`include/`, `lib/`), linked into the binary. Its notices go in with them (`notices`).
#[cfg(feature = "server")]
fn static_ffmpeg(build: &mut cc::Build) {
    println!("cargo:rerun-if-env-changed=ZAPPER_FFMPEG_STATIC");
    let dir = PathBuf::from(std::env::var_os("ZAPPER_FFMPEG_STATIC").unwrap_or_else(|| {
        panic!("a musl zapper-server links a static FFmpeg: run scripts/build-ffmpeg-static.sh and name its folder in ZAPPER_FFMPEG_STATIC (scripts/build-server-static.sh does both)")
    }));
    let lib = dir.join("lib");
    for a in ["libavformat.a", "libavcodec.a", "libswresample.a", "libavutil.a"] {
        let p = lib.join(a);
        assert!(p.is_file(), "{} is missing", p.display());
        println!("cargo:rerun-if-changed={}", p.display());
    }
    build.include(dir.join("include"));
    build.compile("zapper_remux");
    println!("cargo:rustc-link-search=native={}", lib.display());
    // After the remuxer's archive, and in the order they need each other.
    for l in ["avformat", "avcodec", "swresample", "avutil"] {
        println!("cargo:rustc-link-lib=static={l}");
    }
}

/// The licence notices `zapper-server --licenses` prints: every `.txt` in the folder
/// `ZAPPER_SERVER_NOTICES` names, in name order, compiled in, for a binary that travels without
/// the files the packages install beside it (the static one). Without it, `--licenses` says where
/// the packages keep them; a release build for musl refuses.
#[cfg(feature = "server")]
fn notices() {
    println!("cargo:rerun-if-env-changed=ZAPPER_SERVER_NOTICES");
    let mut text = String::new();
    if let Some(dir) = std::env::var_os("ZAPPER_SERVER_NOTICES").map(PathBuf::from) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("ZAPPER_SERVER_NOTICES: {}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "txt"))
            .collect();
        files.sort();
        for f in files {
            println!("cargo:rerun-if-changed={}", f.display());
            let body = std::fs::read_to_string(&f)
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            assert!(!body.trim().is_empty(), "{} is empty", f.display());
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            text.push_str(&format!("{}\n{name}\n{}\n\n{}\n\n", "=".repeat(78), "=".repeat(78), body.trim_end()));
        }
    } else if musl() && std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!("a release zapper-server for musl carries its licence notices: name their folder in ZAPPER_SERVER_NOTICES (scripts/build-server-static.sh does)");
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("notices.txt");
    std::fs::write(&out, text).expect("writing notices.txt");
}

fn copy_if_stale(src: &Path, dst: &Path) {
    let stale = match (std::fs::metadata(src), std::fs::metadata(dst)) {
        (Ok(s), Ok(d)) => s.len() != d.len() || s.modified().ok() > d.modified().ok(),
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if !stale {
        return;
    }
    let copied = dst
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::copy(src, dst).map(|_| ()));
    if let Err(e) = copied {
        println!("cargo:warning=could not copy {} to {}: {e}", src.display(), dst.display());
    }
}
