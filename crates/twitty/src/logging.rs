//! The log file. Release builds have no console, so this is where their logs
//! (and panics) go: `logs\twitty.log` in the data folder (see [`AppDirs`]),
//! rotated once to `twitty.log.1` past [`MAX_BYTES`].
//!
//! Logs never contain secrets: credentials are redacted at the type level
//! (`Secret`, `Credentials`), see CLAUDE.md.

use crate::dirs::AppDirs;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

/// Size past which the log is rotated at startup.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Default location of the log file.
pub fn default_path() -> Option<PathBuf> {
    AppDirs::current().map(|d| d.data.join("logs").join("twitty.log"))
}

/// Opens the log for appending. A log over `max_bytes` first replaces the
/// previous rotation (`<name>.1`), so at most two files exist.
pub fn open(path: &Path, max_bytes: u64) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > max_bytes) {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        fs::rename(path, PathBuf::from(rotated))?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Logs panics (message and location) before the default handler runs, so a
/// crash in a release build leaves a trace in the log file.
pub fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn appends_and_rotates_once_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("twitty.log");
        writeln!(open(&path, 10).unwrap(), "first").unwrap();
        writeln!(open(&path, 1000).unwrap(), "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n", "appends");

        // 13 bytes > 10: rotated before writing.
        writeln!(open(&path, 10).unwrap(), "third").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        let rotated = dir.path().join("logs").join("twitty.log.1");
        assert_eq!(fs::read_to_string(&rotated).unwrap(), "first\nsecond\n");

        // A second rotation replaces the first: never more than two files.
        writeln!(open(&path, 10).unwrap(), "fourth fourth").unwrap();
        writeln!(open(&path, 10).unwrap(), "fifth").unwrap();
        assert_eq!(fs::read_to_string(&rotated).unwrap(), "third\nfourth fourth\n");
        assert_eq!(fs::read_dir(dir.path().join("logs")).unwrap().count(), 2);
    }
}
