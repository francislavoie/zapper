use std::path::{Path, PathBuf};

// `bundle()`, `without_bundle()`, `musl()`: shared with zapper-mpv's build script.
include!("../../packaging/libmpv/bundle.rs");

fn main() {
    let bundle = bundle();
    remuxer(bundle.as_deref());
    web_assets();
    notices();
    // Windows (MSVC) only.
    static_vcruntime::metabuild();
}

/// The web interface zapper-server serves (`recording::web`): the frontend's build in `../../dist`,
/// compiled into the binary. One file to install and
/// nothing beside it to go missing or out of step with the API it talks to.
///
/// Writes `$OUT_DIR/web_assets.rs`, a list of `(path, include_bytes!(…))`; `include_bytes!`
/// makes a changed file rebuild, and watching `dist` notices a file added or removed. A release
/// build refuses without the frontend built (`npm run build`); a debug build serves a page
/// saying so, so the Rust tests and harnesses run without Node.
fn web_assets() {
    use std::fmt::Write as _;
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist");
    // Made when missing, so there is a directory to watch: watching a path that does not exist
    // reruns this script, and rebuilds the crate, on every build. Vite empties and fills it.
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        collect(&dist, &dist, &mut files);
    } else if std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!("zapper-server serves the web interface from ../../dist: run `npm run build` first");
    } else {
        println!("cargo:warning=../../dist has no index.html: zapper-server's web interface will say it was not built (run `npm run build`)");
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

/// Compiles the recording service's remuxers (`src/recording/remux.c`, a capture into Matroska,
/// `src/recording/livemux.c`, a live feed for a browser, and `src/recording/vodmux.c`, a movie file
/// for a browser) against FFmpeg's libavformat,
/// libavcodec, libswresample and libavutil: the bundle's (their headers in `include/`), or, with
/// none fetched, whatever pkg-config finds. The C side does only what needs FFmpeg's structures
/// (demuxing, decoding, encoding and muxing); every decision about timing and naming is Rust's.
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

/// The static zapper-server's FFmpeg: the static libraries `scripts/build-ffmpeg-static.sh` made
/// (only what the service uses, LGPL, docs/licensing.md), in the folder `ZAPPER_FFMPEG_STATIC`
/// names (`include/`, `lib/`), linked into the binary. Its notices go in with them (`notices`).
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
