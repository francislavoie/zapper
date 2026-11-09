//! The log on disk: `zapper.log` for the app, `zapper-server.log` for the
//! recording service, in the data folder ([`crate::paths::data_dir`]).
//!
//! A window has nowhere visible to print, and a sync that fails at three in
//! the morning has to be explainable the next day: the app's log used to go
//! to standard error alone, which on Windows is nowhere at all. So both
//! programs keep a file, and keep it small: past [`MAX_BYTES`] the file is
//! moved aside as `<name>.old.log`, replacing the one before, and a new one
//! started. Two files of a few megabytes at most, whatever the uptime.
//!
//! What is logged is already fit to keep: nothing in zapper logs an account
//! value, and failures are scrubbed of what a URL carries before they are
//! logged (`catalog::sync_engine::scrub`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

/// How large a log may grow before it is moved aside.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A log file that moves itself aside when it is full.
pub struct RotatingFile {
    path: PathBuf,
    old: PathBuf,
    file: Option<File>,
    len: u64,
    max: u64,
}

impl RotatingFile {
    /// `<dir>/<name>.log`, appended to, with `<dir>/<name>.old.log` as the
    /// one before. `None` when the folder cannot be made or the file opened:
    /// logging somewhere is not a reason not to start.
    pub fn open(dir: PathBuf, name: &str) -> Option<Self> {
        Self::open_with(dir, name, MAX_BYTES)
    }

    fn open_with(dir: PathBuf, name: &str, max: u64) -> Option<Self> {
        std::fs::create_dir_all(&dir).ok()?;
        let mut f = Self {
            path: dir.join(format!("{name}.log")),
            old: dir.join(format!("{name}.old.log")),
            file: None,
            len: 0,
            max,
        };
        if std::fs::metadata(&f.path).is_ok_and(|m| m.len() > max) {
            let _ = std::fs::rename(&f.path, &f.old);
        }
        f.reopen().ok()?;
        Some(f)
    }

    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.len = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    /// Moves the full file aside and starts another. Closed first: Windows
    /// will not rename a file this process has open.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        // A failed rename (another program holding the file) leaves the file
        // where it was, to be appended to: a log that grows is better than
        // none.
        let _ = std::fs::rename(&self.path, &self.old);
        self.reopen()
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len + buf.len() as u64 > self.max {
            self.rotate()?;
        }
        let file = match self.file.as_mut() {
            Some(f) => f,
            None => {
                self.reopen()?;
                self.file
                    .as_mut()
                    .ok_or_else(|| io::Error::other("no log file"))?
            }
        };
        let n = file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// Logs at `info` and above to `<data>/<name>.log`, and to standard error
/// as well when `stderr` (a terminal may be watching). Only standard error
/// when the file cannot be opened.
pub fn init(name: &str, stderr: bool) {
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;
    let file = RotatingFile::open(crate::paths::data_dir(), name);
    let to_file = file.as_ref().is_some();
    let file_layer = file.map(|f| {
        fmt::layer()
            .with_writer(std::sync::Mutex::new(f))
            .with_ansi(false)
    });
    let stderr_layer = (stderr || !to_file).then(|| fmt::layer().with_writer(io::stderr));
    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(file_layer)
        .with(stderr_layer)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-logfile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_full_log_is_moved_aside_and_a_new_one_started() {
        let d = dir("rotate");
        let mut f = RotatingFile::open_with(d.clone(), "zapper", 64).unwrap();
        f.write_all(&[b'a'; 40]).unwrap();
        f.write_all(&[b'b'; 40]).unwrap();
        f.flush().unwrap();
        assert_eq!(
            std::fs::read(d.join("zapper.old.log")).unwrap(),
            vec![b'a'; 40]
        );
        assert_eq!(std::fs::read(d.join("zapper.log")).unwrap(), vec![b'b'; 40]);
        // Again: the older one is replaced, so there are only ever two.
        f.write_all(&[b'c'; 40]).unwrap();
        assert_eq!(
            std::fs::read(d.join("zapper.old.log")).unwrap(),
            vec![b'b'; 40]
        );
        assert_eq!(std::fs::read(d.join("zapper.log")).unwrap(), vec![b'c'; 40]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_restart_appends_unless_the_log_is_already_full() {
        let d = dir("reopen");
        let mut f = RotatingFile::open_with(d.clone(), "zapper", 64).unwrap();
        f.write_all(b"first run\n").unwrap();
        drop(f);
        let mut f = RotatingFile::open_with(d.clone(), "zapper", 64).unwrap();
        f.write_all(b"second run\n").unwrap();
        drop(f);
        assert_eq!(
            std::fs::read_to_string(d.join("zapper.log")).unwrap(),
            "first run\nsecond run\n"
        );
        std::fs::write(d.join("zapper.log"), [b'x'; 100]).unwrap();
        let f = RotatingFile::open_with(d.clone(), "zapper", 64).unwrap();
        drop(f);
        assert_eq!(std::fs::read(d.join("zapper.old.log")).unwrap().len(), 100);
        assert_eq!(std::fs::read(d.join("zapper.log")).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
