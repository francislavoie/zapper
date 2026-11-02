//! Where zapper keeps things on disk, per platform.
//!
//! Three places, because they are three different kinds of thing:
//!
//! - **config** — the account and the window's size. Small, precious, and
//!   the thing a person backs up. Deleting the cache must not log them out.
//! - **data** — the catalogue database. Rebuildable from the source, but
//!   large and slow to rebuild.
//! - **cache** — downloaded updates. Disposable.
//!
//! | | Linux | Windows | macOS |
//! |---|---|---|---|
//! | config | `$XDG_CONFIG_HOME/zapper`, else `~/.config/zapper` | `%APPDATA%\zapper` | `~/Library/Application Support/zapper` |
//! | data | `~/.local/share/zapper` | `%LOCALAPPDATA%\zapper` | `~/Library/Application Support/zapper` |
//! | cache | `$XDG_CACHE_HOME/zapper`, else `~/.cache/zapper` | `%LOCALAPPDATA%\zapper` | `~/Library/Caches/zapper` |
//!
//! Linux is exactly what zapper used before it ran anywhere else, including
//! its one quirk: the database ignores `XDG_DATA_HOME`. Moving it now would
//! strand every existing catalogue, so it stays.
//!
//! Windows splits config into the roaming profile and data into the local
//! one, which is what those folders are for: an account follows a person to
//! another machine on a domain, a 200MB guide database should not.
//!
//! Plain environment lookups rather than a `dirs` crate: four variables are
//! not worth a dependency, and the rules are visible here rather than
//! inherited.

use std::path::PathBuf;

/// Which platform's rules to apply. A parameter rather than only a `cfg`, so
/// every platform's rules are tested on whichever one runs the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Linux,
    Windows,
    Mac,
}

const OS: Os = if cfg!(windows) {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Mac
} else {
    Os::Linux
};

/// An environment variable as a path; unset and empty are the same thing,
/// as the XDG specification says they are.
fn env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The user's home. Empty when unknown, which leaves a relative path: the
/// behaviour zapper always had on Linux with `HOME` unset, kept rather than
/// replaced with a guess.
fn home(env: &dyn Fn(&str) -> Option<PathBuf>, os: Os) -> PathBuf {
    match os {
        Os::Windows => env("USERPROFILE").unwrap_or_default(),
        _ => env("HOME").unwrap_or_default(),
    }
}

fn config_dir_for(os: Os, env: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    match os {
        Os::Linux => env("XDG_CONFIG_HOME").unwrap_or_else(|| home(env, os).join(".config")),
        Os::Windows => env("APPDATA").unwrap_or_else(|| home(env, os).join("AppData/Roaming")),
        Os::Mac => home(env, os).join("Library/Application Support"),
    }
    .join("zapper")
}

fn data_dir_for(os: Os, env: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    match os {
        // Not XDG_DATA_HOME: see the module comment.
        Os::Linux => home(env, os).join(".local/share"),
        Os::Windows => env("LOCALAPPDATA").unwrap_or_else(|| home(env, os).join("AppData/Local")),
        Os::Mac => home(env, os).join("Library/Application Support"),
    }
    .join("zapper")
}

fn cache_dir_for(os: Os, env: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    match os {
        Os::Linux => env("XDG_CACHE_HOME").unwrap_or_else(|| home(env, os).join(".cache")),
        // The cache is local by nature; Windows has no separate cache root.
        Os::Windows => env("LOCALAPPDATA").unwrap_or_else(|| home(env, os).join("AppData/Local")),
        Os::Mac => home(env, os).join("Library/Caches"),
    }
    .join("zapper")
}

/// The variable that moves every directory at once; see [`root_override`].
pub const HOME_OVERRIDE: &str = "ZAPPER_HOME";

/// Every directory at once, moved somewhere else: `ZAPPER_HOME=<dir>` keeps
/// config, data and cache in `<dir>/config`, `<dir>/data` and `<dir>/cache`.
///
/// One variable rather than the platform's three, because the point is to be
/// sure: a run pointed here can neither read the account on the machine nor
/// write into its catalogue, whatever the platform's rules would have said.
/// Harnesses and unattended runs set it (see [`crate::harness`]).
///
/// Unit tests never get the real directories at all: each test thread sees
/// the directory a test installed with [`with_test_root`], or else one
/// sandbox for the whole test process under the system's temporary
/// directory. That is a property of the build, not of the environment the
/// tests happen to be run from, so nothing a test calls can reach the
/// account or the catalogue of whoever is running them.
fn root_override() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(test_root())
    }
    #[cfg(not(test))]
    {
        env(HOME_OVERRIDE)
    }
}

/// Each directory on its own, named exactly: `ZAPPER_CONFIG_DIR`,
/// `ZAPPER_DATA_DIR` and `ZAPPER_CACHE_DIR` are the directories themselves,
/// not a root to put `zapper` in. For a process that runs as another
/// account than the person whose zapper it serves: the recording service
/// installed as a Windows service at boot runs as an account of its own,
/// whose `%APPDATA%` is not theirs, and is handed theirs this way
/// (`zapper-server --config-dir` and the rest). [`HOME_OVERRIDE`] comes
/// first.
pub const CONFIG_DIR_OVERRIDE: &str = "ZAPPER_CONFIG_DIR";
pub const DATA_DIR_OVERRIDE: &str = "ZAPPER_DATA_DIR";
pub const CACHE_DIR_OVERRIDE: &str = "ZAPPER_CACHE_DIR";

/// Where the account and window state live.
pub fn config_dir() -> PathBuf {
    match root_override() {
        Some(root) => root.join("config"),
        None => env(CONFIG_DIR_OVERRIDE).unwrap_or_else(|| config_dir_for(OS, &env)),
    }
}

/// Where the catalogue database lives.
pub fn data_dir() -> PathBuf {
    match root_override() {
        Some(root) => root.join("data"),
        None => env(DATA_DIR_OVERRIDE).unwrap_or_else(|| data_dir_for(OS, &env)),
    }
}

/// Where disposable downloads live.
pub fn cache_dir() -> PathBuf {
    match root_override() {
        Some(root) => root.join("cache"),
        None => env(CACHE_DIR_OVERRIDE).unwrap_or_else(|| cache_dir_for(OS, &env)),
    }
}

#[cfg(test)]
thread_local! {
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// See [`root_override`]: this thread's installed root, else the process's
/// sandbox. Never the real directories.
#[cfg(test)]
fn test_root() -> PathBuf {
    TEST_ROOT.with(|r| r.borrow().clone()).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("zapper-unit-tests-{}", std::process::id()))
    })
}

/// Runs `f` with this thread's directories under `root`.
///
/// Per thread rather than through the environment: `set_var` is
/// process-wide, so a test that moved the config directory that way moved it
/// for every other test running at that moment too, and moved it back
/// underneath them.
#[cfg(test)]
pub fn with_test_root<T>(root: &std::path::Path, f: impl FnOnce() -> T) -> T {
    /// Puts the previous root back on unwind as well, so a failing test does
    /// not leave the next test on this thread in its directory.
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_ROOT.with(|r| *r.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_ROOT.with(|r| r.replace(Some(root.to_path_buf()))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<PathBuf> {
        move |k| {
            vars.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| PathBuf::from(v))
                .filter(|p| !p.as_os_str().is_empty())
        }
    }

    #[test]
    fn linux_is_unchanged_from_before_there_were_other_platforms() {
        let e = with(&[("HOME", "/home/u")]);
        assert_eq!(config_dir_for(Os::Linux, &e), PathBuf::from("/home/u/.config/zapper"));
        assert_eq!(data_dir_for(Os::Linux, &e), PathBuf::from("/home/u/.local/share/zapper"));
        assert_eq!(cache_dir_for(Os::Linux, &e), PathBuf::from("/home/u/.cache/zapper"));
    }

    #[test]
    fn linux_honours_xdg_config_and_cache_but_not_data() {
        let e = with(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_CACHE_HOME", "/cache"),
            ("XDG_DATA_HOME", "/data"),
        ]);
        assert_eq!(config_dir_for(Os::Linux, &e), PathBuf::from("/cfg/zapper"));
        assert_eq!(cache_dir_for(Os::Linux, &e), PathBuf::from("/cache/zapper"));
        assert_eq!(data_dir_for(Os::Linux, &e), PathBuf::from("/home/u/.local/share/zapper"));
    }

    #[test]
    fn an_empty_xdg_variable_counts_as_unset() {
        let e = with(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(config_dir_for(Os::Linux, &e), PathBuf::from("/home/u/.config/zapper"));
    }

    #[test]
    fn windows_splits_roaming_config_from_local_data() {
        let e = with(&[
            ("USERPROFILE", r"C:\Users\u"),
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ]);
        assert_eq!(
            config_dir_for(Os::Windows, &e),
            PathBuf::from(r"C:\Users\u\AppData\Roaming").join("zapper")
        );
        assert_eq!(
            data_dir_for(Os::Windows, &e),
            PathBuf::from(r"C:\Users\u\AppData\Local").join("zapper")
        );
        assert_eq!(cache_dir_for(Os::Windows, &e), data_dir_for(Os::Windows, &e));
    }

    #[test]
    fn windows_without_appdata_falls_back_under_the_profile() {
        let e = with(&[("USERPROFILE", "/profile")]);
        assert_eq!(
            config_dir_for(Os::Windows, &e),
            PathBuf::from("/profile/AppData/Roaming/zapper")
        );
        assert_eq!(data_dir_for(Os::Windows, &e), PathBuf::from("/profile/AppData/Local/zapper"));
    }

    #[test]
    fn unit_tests_never_see_the_real_directories() {
        let real = [
            config_dir_for(OS, &env),
            data_dir_for(OS, &env),
            cache_dir_for(OS, &env),
        ];
        for d in [config_dir(), data_dir(), cache_dir()] {
            assert!(d.starts_with(std::env::temp_dir()), "{}", d.display());
            assert!(!real.contains(&d), "{} is a real directory", d.display());
        }
    }

    #[test]
    fn a_test_root_moves_every_directory_and_is_put_back() {
        let before = config_dir();
        let root = std::path::Path::new("/nowhere/zapper-test-root");
        with_test_root(root, || {
            assert_eq!(config_dir(), root.join("config"));
            assert_eq!(data_dir(), root.join("data"));
            assert_eq!(cache_dir(), root.join("cache"));
        });
        assert_eq!(config_dir(), before);
    }

    #[test]
    fn the_mac_uses_application_support_and_caches() {
        let e = with(&[("HOME", "/Users/u")]);
        assert_eq!(
            config_dir_for(Os::Mac, &e),
            PathBuf::from("/Users/u/Library/Application Support/zapper")
        );
        assert_eq!(data_dir_for(Os::Mac, &e), config_dir_for(Os::Mac, &e));
        assert_eq!(
            cache_dir_for(Os::Mac, &e),
            PathBuf::from("/Users/u/Library/Caches/zapper")
        );
    }
}
