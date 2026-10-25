//! Where Twitty keeps its files.
//!
//! Release builds use the per-user folders (`%APPDATA%\Twitty\config`,
//! `%LOCALAPPDATA%\Twitty\cache`, `%LOCALAPPDATA%\Twitty\data`). Debug builds
//! use `dev-data/` at the repository root instead, so development never reads
//! or overwrites the installed app's decks, settings, caches or logs.

use std::path::{Path, PathBuf};

/// The three folders the app writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Decks, settings, the development `session.json`.
    pub config: PathBuf,
    /// Query ids, client ids, the sign-in window's WebView2 profile, downloads.
    pub cache: PathBuf,
    /// Logs.
    pub data: PathBuf,
}

impl AppDirs {
    /// The folders of this build: `dev-data/` in debug, per-user in release.
    pub fn current() -> Option<Self> {
        if cfg!(debug_assertions) { Some(Self::under(&dev_root())) } else { Self::per_user() }
    }

    /// The folders under `root`.
    pub fn under(root: &Path) -> Self {
        Self { config: root.join("config"), cache: root.join("cache"), data: root.join("data") }
    }

    fn per_user() -> Option<Self> {
        directories::ProjectDirs::from("", "", "Twitty").map(|d| Self {
            config: d.config_dir().to_path_buf(),
            cache: d.cache_dir().to_path_buf(),
            data: d.data_local_dir().to_path_buf(),
        })
    }
}

/// `dev-data/` at the repository root (this crate is `crates/twitty`).
fn dev_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).join("dev-data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn debug_builds_use_dev_data_at_the_repo_root() {
        let root = dev_root();
        assert_eq!(root.file_name().and_then(|n| n.to_str()), Some("dev-data"));
        assert!(root.parent().unwrap().join("Cargo.lock").is_file(), "{}", root.display());
        // Tests are debug builds.
        assert_eq!(AppDirs::current(), Some(AppDirs::under(&root)));
    }

    #[test]
    fn folders_sit_side_by_side_under_the_root() {
        let dirs = AppDirs::under(Path::new("r"));
        assert_eq!(
            (dirs.config, dirs.cache, dirs.data),
            (PathBuf::from("r/config"), PathBuf::from("r/cache"), PathBuf::from("r/data"))
        );
    }
}
