//! User settings and their on-disk format (`settings.json`).
//!
//! Versioned with `schema_version` like the deck file; golden files for every
//! version live in `tests/golden/`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    dirs::AppDirs,
    state::{media_layout::MaxHeight, playback::Autoplay},
};

/// Current version of the settings file.
pub const SCHEMA_VERSION: u32 = 1;

/// Everything stored in `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub schema_version: u32,
    /// Theme name; unknown names fall back to the default theme.
    #[serde(default)]
    pub theme: Option<String>,
    /// Whether columns check for new tweets on their own (default on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling: Option<bool>,
    /// Whether the UI animates (default on); off renders every animation at
    /// rest (GPUI's reduce motion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<bool>,
    /// Whether GIFs in timelines play as soon as they are on screen (default
    /// off: they play when clicked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gif_autoplay: Option<bool>,
    /// The tallest a tweet's media may be (default: no limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_max_height: Option<MaxHeight>,
    /// Whether the sidebar shows labels and the decks' edit buttons (default
    /// off: icons only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_expanded: Option<bool>,
    /// Whether Twitty looks for new releases on GitHub (default on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_updates: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            theme: None,
            polling: None,
            animations: None,
            gif_autoplay: None,
            media_max_height: None,
            sidebar_expanded: None,
            check_updates: None,
        }
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

    pub fn polling_enabled(&self) -> bool {
        self.polling.unwrap_or(true)
    }

    pub fn animations_enabled(&self) -> bool {
        self.animations.unwrap_or(true)
    }

    pub fn check_updates(&self) -> bool {
        self.check_updates.unwrap_or(true)
    }

    pub fn sidebar_expanded(&self) -> bool {
        self.sidebar_expanded.unwrap_or(false)
    }

    pub fn gif_autoplay(&self) -> Autoplay {
        if self.gif_autoplay.unwrap_or(false) { Autoplay::On } else { Autoplay::Off }
    }

    /// The saved theme if it is one of `known`, else the first of `known`.
    pub fn theme_or_default<'a>(&self, known: &[&'a str]) -> Option<&'a str> {
        let saved = self.theme.as_deref();
        known.iter().copied().find(|k| Some(*k) == saved).or_else(|| known.first().copied())
    }
}

/// Default location: `settings.json` in the config folder (see [`AppDirs`]).
pub fn default_path() -> Option<PathBuf> {
    AppDirs::current().map(|d| d.config.join("settings.json"))
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
        assert!(s.polling_enabled(), "v1 files predate the setting: polling on");
        assert!(s.animations_enabled(), "and animations on");
        assert_eq!(s.gif_autoplay(), Autoplay::Off, "and GIFs wait for a click");
        assert_eq!(s.media_max_height, None, "and media are not capped");
        assert!(!s.sidebar_expanded(), "and the sidebar shows icons only");
        assert!(s.check_updates(), "and updates are looked for");
    }

    #[test]
    fn unknown_theme_falls_back() {
        let s = Settings { theme: Some("Neon".into()), ..Settings::default() };
        assert_eq!(s.theme_or_default(&KNOWN), Some("Lights out"));
        assert_eq!(Settings::default().theme_or_default(&KNOWN), Some("Lights out"));
    }

    #[test]
    fn media_max_height_is_stored_as_pixels_and_clamped() {
        let s = Settings { media_max_height: Some(MaxHeight::new(420.0)), ..Settings::default() };
        let json: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(json["media_max_height"], serde_json::json!(420));
        let edited =
            Settings::from_json(r#"{"schema_version": 1, "media_max_height": 5}"#).unwrap();
        assert_eq!(edited.media_max_height, Some(MaxHeight::new(0.0)), "clamped to the minimum");
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
        let s = Settings {
            theme: Some("Light".into()),
            animations: Some(false),
            gif_autoplay: Some(true),
            media_max_height: Some(MaxHeight::new(420.0)),
            sidebar_expanded: Some(true),
            check_updates: Some(false),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
    }
}
