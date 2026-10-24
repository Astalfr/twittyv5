//! User settings and their on-disk format (`settings.json`).
//!
//! Versioned with `schema_version` like the deck file; golden files for every
//! version live in `tests/golden/`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Current version of the settings file.
pub const SCHEMA_VERSION: u32 = 1;

/// Everything stored in `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub schema_version: u32,
    /// Theme name; unknown names fall back to the default theme.
    #[serde(default)]
    pub theme: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { schema_version: SCHEMA_VERSION, theme: None }
    }
}

/// Why the settings file could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings file I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("settings file is invalid: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("settings file version {0} is newer than this app supports ({SCHEMA_VERSION})")]
    TooNew(u32),
}

impl Settings {
    /// Parses a settings file of any supported version.
    pub fn from_json(json: &str) -> Result<Self, SettingsError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let version = value.get("schema_version").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let version = u32::try_from(version).unwrap_or(u32::MAX);
        if version > SCHEMA_VERSION {
            return Err(SettingsError::TooNew(version));
        }
        // Migrations from older versions go here, one step at a time.
        let mut settings: Settings = serde_json::from_value(value)?;
        settings.schema_version = SCHEMA_VERSION;
        Ok(settings)
    }

    /// Loads from `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match fs::read_to_string(path) {
            Ok(json) => Self::from_json(&json),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Saves atomically (write to a temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The saved theme if it is one of `known`, else the first of `known`.
    pub fn theme_or_default<'a>(&self, known: &[&'a str]) -> Option<&'a str> {
        let saved = self.theme.as_deref();
        known.iter().copied().find(|k| Some(*k) == saved).or_else(|| known.first().copied())
    }
}

/// Default location: `%APPDATA%\Twitty\config\settings.json` on Windows.
pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Twitty").map(|d| d.config_dir().join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const KNOWN: [&str; 3] = ["Lights out", "Dim", "Light"];

    #[test]
    fn golden_v1_still_loads() {
        let s = Settings::from_json(include_str!("../../tests/golden/settings_v1.json")).unwrap();
        assert_eq!(s.theme.as_deref(), Some("Dim"));
        assert_eq!(s.theme_or_default(&KNOWN), Some("Dim"));
    }

    #[test]
    fn unknown_theme_falls_back() {
        let s = Settings { theme: Some("Neon".into()), ..Settings::default() };
        assert_eq!(s.theme_or_default(&KNOWN), Some("Lights out"));
        assert_eq!(Settings::default().theme_or_default(&KNOWN), Some("Lights out"));
    }

    #[test]
    fn newer_versions_are_refused() {
        assert!(matches!(
            Settings::from_json(r#"{"schema_version": 2}"#),
            Err(SettingsError::TooNew(2))
        ));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default(), "missing file");
        let s = Settings { theme: Some("Light".into()), ..Settings::default() };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
    }
}
