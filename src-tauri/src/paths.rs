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

/// Where the account and window state live.
pub fn config_dir() -> PathBuf {
    config_dir_for(OS, &env)
}

/// Where the catalogue database lives.
pub fn data_dir() -> PathBuf {
    data_dir_for(OS, &env)
}

/// Where disposable downloads live.
pub fn cache_dir() -> PathBuf {
    cache_dir_for(OS, &env)
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
