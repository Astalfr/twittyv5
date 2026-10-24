//! Decks (workspaces of columns) and their on-disk format.
//!
//! The file is versioned with `schema_version`; older versions are migrated
//! on load. Golden files for every version live in `tests/golden/`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use twitty_model::{FeedSource, HomeMode, NotificationFilter, SearchSort};

/// Current version of the deck file.
///
/// - v1: widths were presets (`"narrow"`, `"medium"`, `"wide"`).
/// - v2: widths are pixels; columns gained a name and alert settings.
pub const SCHEMA_VERSION: u32 = 2;

/// Stable identifier of a column within a deck file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u64);

/// Column width in logical pixels, always within [`Self::MIN`]..=[`Self::MAX`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u16", into = "u16")]
pub struct ColumnWidth(u16);

impl ColumnWidth {
    pub const MIN: u16 = 280;
    pub const MAX: u16 = 640;
    /// Slider granularity.
    pub const STEP: u16 = 10;

    /// `px` rounded to the nearest pixel and clamped to the allowed range.
    pub fn new(px: f32) -> Self {
        let px = if px.is_finite() { px.round() } else { 0.0 };
        // In range after the clamp, so the cast is exact.
        Self(px.clamp(f32::from(Self::MIN), f32::from(Self::MAX)) as u16)
    }

    /// Width in logical pixels.
    pub fn px(self) -> f32 {
        f32::from(self.0)
    }

    /// The width a v1 preset name stood for.
    fn from_v1_preset(name: &str) -> Self {
        match name {
            "narrow" => Self(310),
            "wide" => Self(400),
            _ => Self::default(),
        }
    }
}

impl Default for ColumnWidth {
    fn default() -> Self {
        Self(350)
    }
}

impl From<u16> for ColumnWidth {
    fn from(px: u16) -> Self {
        Self(px.clamp(Self::MIN, Self::MAX))
    }
}

impl From<ColumnWidth> for u16 {
    fn from(width: ColumnWidth) -> Self {
        width.0
    }
}

/// Loudness of a column's sound, in percent (0..=100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub struct Volume(u8);

impl Volume {
    pub const MAX: u8 = 100;

    /// `percent` rounded and clamped to 0..=100.
    pub fn new(percent: f32) -> Self {
        let percent = if percent.is_finite() { percent.round() } else { 0.0 };
        // In range after the clamp, so the cast is exact.
        Self(percent.clamp(0.0, f32::from(Self::MAX)) as u8)
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    /// The gain to play at: 0.0 (silent) to 1.0 (the file's own level).
    pub fn gain(self) -> f32 {
        f32::from(self.0) / f32::from(Self::MAX)
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self(80)
    }
}

impl From<u8> for Volume {
    fn from(percent: u8) -> Self {
        Self(percent.min(Self::MAX))
    }
}

impl From<Volume> for u8 {
    fn from(volume: Volume) -> Self {
        volume.0
    }
}

/// How a column announces new posts that arrive while it polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColumnAlerts {
    /// Play the notification sound.
    #[serde(default)]
    pub sound: bool,
    /// How loud the sound plays.
    #[serde(default)]
    pub volume: Volume,
    /// Show a system (Windows) notification.
    #[serde(default)]
    pub desktop: bool,
}

/// What the user can change in a column's settings dialog.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ColumnSettings {
    /// Shown in place of the source's title; never blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub width: ColumnWidth,
    #[serde(default)]
    pub alerts: ColumnAlerts,
}

impl ColumnSettings {
    /// Sets the name; blank text clears it.
    pub fn set_name(&mut self, name: &str) {
        let name = name.trim();
        self.name = (!name.is_empty()).then(|| name.to_owned());
    }

    /// The column's title: its name, else the source's title.
    pub fn title(&self, source: &FeedSource) -> String {
        self.name.clone().unwrap_or_else(|| source.title())
    }
}

/// One column of a deck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnConfig {
    pub id: ColumnId,
    pub source: FeedSource,
    #[serde(flatten)]
    pub settings: ColumnSettings,
}

impl ColumnConfig {
    /// A column with default settings.
    pub fn new(id: ColumnId, source: FeedSource) -> Self {
        Self { id, source, settings: ColumnSettings::default() }
    }
}

/// A named set of columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deck {
    pub name: String,
    pub columns: Vec<ColumnConfig>,
}

/// Everything stored in `decks.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeckFile {
    pub schema_version: u32,
    pub decks: Vec<Deck>,
    #[serde(default)]
    pub active: usize,
    /// Next id to hand out; never reused.
    pub next_column_id: u64,
}

/// Why the deck file could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum DeckError {
    #[error("deck file I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("deck file is invalid: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("deck file version {0} is newer than this app supports ({SCHEMA_VERSION})")]
    TooNew(u32),
}

impl Default for DeckFile {
    fn default() -> Self {
        let mut file = DeckFile {
            schema_version: SCHEMA_VERSION,
            decks: vec![Deck { name: "Main".into(), columns: Vec::new() }],
            active: 0,
            next_column_id: 1,
        };
        for source in [
            FeedSource::Home { mode: HomeMode::Following },
            FeedSource::Notifications { filter: NotificationFilter::All },
            FeedSource::Search { query: "rust gpui".into(), sort: SearchSort::Latest },
            FeedSource::Home { mode: HomeMode::ForYou },
        ] {
            file.add_column(source);
        }
        file
    }
}

impl DeckFile {
    /// Parses a deck file of any supported version.
    pub fn from_json(json: &str) -> Result<Self, DeckError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let version = value.get("schema_version").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let version = u32::try_from(version).unwrap_or(u32::MAX);
        if version > SCHEMA_VERSION {
            return Err(DeckError::TooNew(version));
        }
        // Migrations from older versions, one step at a time.
        let mut value = value;
        if version < 2 {
            migrate_v1(&mut value);
        }
        let mut file: DeckFile = serde_json::from_value(value)?;
        file.schema_version = SCHEMA_VERSION;
        file.repair();
        Ok(file)
    }

    /// Serializes to pretty JSON.
    pub fn to_json(&self) -> Result<String, DeckError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Loads from `path`, falling back to the default deck when the file does not exist.
    pub fn load(path: &Path) -> Result<Self, DeckError> {
        match fs::read_to_string(path) {
            Ok(json) => Self::from_json(&json),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Saves atomically (write to a temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), DeckError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, self.to_json()?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The active deck.
    pub fn active(&self) -> &Deck {
        // `repair` guarantees at least one deck and a valid index.
        &self.decks[self.active.min(self.decks.len().saturating_sub(1))]
    }

    fn active_mut(&mut self) -> &mut Deck {
        let ix = self.active.min(self.decks.len().saturating_sub(1));
        &mut self.decks[ix]
    }

    /// Appends a column to the active deck.
    pub fn add_column(&mut self, source: FeedSource) -> ColumnId {
        let id = ColumnId(self.next_column_id);
        self.next_column_id += 1;
        self.active_mut().columns.push(ColumnConfig::new(id, source));
        id
    }

    /// Removes a column from the active deck. Returns whether it existed.
    pub fn remove_column(&mut self, id: ColumnId) -> bool {
        let cols = &mut self.active_mut().columns;
        let before = cols.len();
        cols.retain(|c| c.id != id);
        cols.len() != before
    }

    /// Moves a column by `delta` positions, clamped to the deck.
    pub fn move_column(&mut self, id: ColumnId, delta: isize) {
        let cols = &mut self.active_mut().columns;
        if let Some(from) = cols.iter().position(|c| c.id == id) {
            let to = from.saturating_add_signed(delta).min(cols.len() - 1);
            let col = cols.remove(from);
            cols.insert(to, col);
        }
    }

    /// Index of the active deck.
    pub fn active_index(&self) -> usize {
        self.active.min(self.decks.len().saturating_sub(1))
    }

    /// Adds an empty deck named "Deck N" and makes it active. Returns its index.
    pub fn add_deck(&mut self) -> usize {
        // The lowest free number; "Main" counts as deck 1.
        let n = (2..)
            .find(|n| !self.decks.iter().any(|d| d.name == format!("Deck {n}")))
            .unwrap_or(self.decks.len() + 1);
        self.decks.push(Deck { name: format!("Deck {n}"), columns: Vec::new() });
        self.active = self.decks.len() - 1;
        self.active
    }

    /// Switches to deck `index`. Returns whether it exists.
    pub fn set_active(&mut self, index: usize) -> bool {
        let exists = index < self.decks.len();
        if exists {
            self.active = index;
        }
        exists
    }

    /// Removes deck `index` (never the last one). The active deck stays the
    /// same deck when possible, else its left neighbour becomes active.
    pub fn remove_deck(&mut self, index: usize) -> bool {
        if self.decks.len() <= 1 || index >= self.decks.len() {
            return false;
        }
        let active = self.active_index();
        self.decks.remove(index);
        if index < active || (index == active && active > 0) {
            self.active = active - 1;
        }
        self.active = self.active_index();
        true
    }

    /// Mutable access to one column's settings.
    pub fn column_mut(&mut self, id: ColumnId) -> Option<&mut ColumnConfig> {
        self.active_mut().columns.iter_mut().find(|c| c.id == id)
    }

    /// Fixes inconsistencies a hand-edited or partially written file may contain.
    fn repair(&mut self) {
        if self.decks.is_empty() {
            self.decks.push(Deck { name: "Main".into(), columns: Vec::new() });
        }
        if self.active >= self.decks.len() {
            self.active = 0;
        }
        let max_id = self.decks.iter().flat_map(|d| &d.columns).map(|c| c.id.0).max().unwrap_or(0);
        self.next_column_id = self.next_column_id.max(max_id + 1);
    }
}

/// v1 -> v2: preset widths become pixels. Everything else is unchanged; the
/// new fields take their defaults.
fn migrate_v1(value: &mut serde_json::Value) {
    let columns = value
        .get_mut("decks")
        .and_then(serde_json::Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(|deck| deck.get_mut("columns").and_then(serde_json::Value::as_array_mut))
        .flatten();
    for column in columns {
        if let Some(width) = column.get_mut("width") {
            let preset = width.as_str().unwrap_or_default();
            *width = u16::from(ColumnWidth::from_v1_preset(preset)).into();
        }
    }
}

/// Default location: `%APPDATA%\Twitty\decks.json` on Windows.
pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Twitty").map(|d| d.config_dir().join("decks.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn decks_can_be_added_switched_and_removed() {
        let mut file = DeckFile::default();
        let main_columns = file.active().columns.len();
        assert_eq!(file.add_deck(), 1);
        assert_eq!(file.active().name, "Deck 2");
        assert!(file.active().columns.is_empty());
        let id = file.add_column(FeedSource::Bookmarks);
        assert_eq!(file.decks[1].columns[0].id, id, "columns go to the active deck");
        assert!(file.set_active(0));
        assert_eq!(file.active().columns.len(), main_columns);
        assert!(!file.set_active(5));

        // Removing a deck left of the active one keeps the same deck active.
        file.add_deck();
        file.set_active(2);
        assert!(file.remove_deck(0));
        assert_eq!(file.active().name, "Deck 3");
        // Removing the active deck activates its left neighbour.
        assert!(file.remove_deck(1));
        assert_eq!(file.active().name, "Deck 2");
        assert!(!file.remove_deck(0), "the last deck stays");
    }

    #[test]
    fn deck_names_stay_unique() {
        let mut file = DeckFile::default();
        file.add_deck();
        file.add_deck();
        file.remove_deck(1);
        file.add_deck();
        let names: Vec<_> = file.decks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Main", "Deck 3", "Deck 2"]);
    }

    #[test]
    fn default_deck_has_columns_and_round_trips() {
        let file = DeckFile::default();
        assert_eq!(file.active().columns.len(), 4);
        let back = DeckFile::from_json(&file.to_json().unwrap()).unwrap();
        assert_eq!(back, file);
    }

    #[test]
    fn golden_v1_loads() {
        let json = include_str!("../../tests/golden/decks_v1.json");
        let file = DeckFile::from_json(json).unwrap();
        assert_eq!(file.schema_version, SCHEMA_VERSION);
        let cols = &file.active().columns;
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0].source, FeedSource::Home { mode: HomeMode::Following });
        // v1 presets migrate to the pixels they stood for.
        assert_eq!(cols[0].settings.width.px(), 350.0, "medium");
        assert_eq!(cols[1].settings.width.px(), 400.0, "wide");
        // Width missing in the file falls back to the default.
        assert_eq!(cols[2].settings.width, ColumnWidth::default());
        assert!(cols.iter().all(|c| c.settings.name.is_none()));
        assert!(cols.iter().all(|c| c.settings.alerts == ColumnAlerts::default()));
    }

    #[test]
    fn narrow_v1_preset_migrates() {
        let json = r#"{"schema_version":1,"decks":[{"name":"A","columns":[{"id":1,"source":{"type":"bookmarks"},"width":"narrow"}]}],"next_column_id":2}"#;
        let file = DeckFile::from_json(json).unwrap();
        assert_eq!(file.active().columns[0].settings.width.px(), 310.0);
    }

    #[test]
    fn golden_v2_loads() {
        let json = include_str!("../../tests/golden/decks_v2.json");
        let file = DeckFile::from_json(json).unwrap();
        let cols = &file.active().columns;
        assert_eq!(cols.len(), 3);
        let named = &cols[1];
        assert_eq!(named.settings.name.as_deref(), Some("Rust news"));
        assert_eq!(named.settings.title(&named.source), "Rust news");
        assert_eq!(named.settings.width.px(), 420.0);
        assert_eq!(
            named.settings.alerts,
            ColumnAlerts { sound: true, volume: Volume::from(35), desktop: true }
        );
        assert_eq!(cols[2].settings, ColumnSettings::default(), "every field is optional");
        assert_eq!(cols[2].settings.title(&cols[2].source), "Notifications");

        // Saving writes the same shape back.
        let saved: serde_json::Value = serde_json::from_str(&file.to_json().unwrap()).unwrap();
        let golden: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(saved["decks"][0]["columns"][1], golden["decks"][0]["columns"][1]);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let json = r#"{"schema_version":2,"decks":[{"name":"A","columns":[
            {"id":1,"source":{"type":"bookmarks"},"width":9999,"alerts":{"volume":250}},
            {"id":2,"source":{"type":"bookmarks"},"width":10}]}],"next_column_id":3}"#;
        let file = DeckFile::from_json(json).unwrap();
        let cols = &file.active().columns;
        assert_eq!(cols[0].settings.width.px(), f32::from(ColumnWidth::MAX));
        assert_eq!(cols[0].settings.alerts.volume.percent(), 100);
        assert_eq!(cols[1].settings.width.px(), f32::from(ColumnWidth::MIN));
    }

    #[test]
    fn newer_version_is_rejected() {
        let err = DeckFile::from_json(r#"{"schema_version":99,"decks":[],"next_column_id":1}"#);
        assert!(matches!(err, Err(DeckError::TooNew(99))));
    }

    #[test]
    fn repair_fixes_bad_indexes_and_ids() {
        let json = r#"{"schema_version":1,"decks":[{"name":"A","columns":[{"id":7,"source":{"type":"bookmarks"}}]}],"active":5,"next_column_id":2}"#;
        let mut file = DeckFile::from_json(json).unwrap();
        assert_eq!(file.active, 0);
        assert_eq!(file.add_column(FeedSource::Bookmarks), ColumnId(8), "ids never collide");
    }

    #[test]
    fn empty_decks_are_repaired() {
        let file =
            DeckFile::from_json(r#"{"schema_version":1,"decks":[],"next_column_id":1}"#).unwrap();
        assert!(file.active().columns.is_empty());
    }

    #[test]
    fn add_remove_move_columns() {
        let mut file = DeckFile::default();
        let ids: Vec<_> = file.active().columns.iter().map(|c| c.id).collect();
        let new = file.add_column(FeedSource::Bookmarks);
        assert_eq!(file.active().columns.last().map(|c| c.id), Some(new));

        file.move_column(new, -10);
        assert_eq!(file.active().columns[0].id, new, "clamped to the start");
        file.move_column(new, 100);
        assert_eq!(file.active().columns.last().map(|c| c.id), Some(new), "clamped to the end");

        assert!(file.remove_column(ids[0]));
        assert!(!file.remove_column(ids[0]), "second removal is a no-op");
        assert_eq!(file.active().columns.len(), 4);
    }

    #[test]
    fn width_and_volume_round_and_clamp() {
        assert_eq!(ColumnWidth::new(333.4).px(), 333.0);
        assert_eq!(ColumnWidth::new(0.0).px(), f32::from(ColumnWidth::MIN));
        assert_eq!(ColumnWidth::new(f32::NAN).px(), f32::from(ColumnWidth::MIN));
        assert_eq!(ColumnWidth::new(1e9).px(), f32::from(ColumnWidth::MAX));
        assert_eq!(Volume::new(49.6).percent(), 50);
        assert_eq!(Volume::new(-3.0).gain(), 0.0);
        assert_eq!(Volume::new(500.0).gain(), 1.0);
    }

    #[test]
    fn blank_names_clear_the_name() {
        let source = FeedSource::Bookmarks;
        let mut settings = ColumnSettings::default();
        settings.set_name("  Saved  ");
        assert_eq!(settings.title(&source), "Saved", "trimmed");
        settings.set_name("   ");
        assert_eq!(settings.name, None);
        assert_eq!(settings.title(&source), "Bookmarks", "falls back to the source");
    }

    #[test]
    fn save_then_load_is_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("decks.json");
        let mut file = DeckFile::default();
        file.add_column(FeedSource::Bookmarks);
        file.save(&path).unwrap();
        assert_eq!(DeckFile::load(&path).unwrap(), file);
    }

    #[test]
    fn missing_file_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(DeckFile::load(&dir.path().join("none.json")).unwrap(), DeckFile::default());
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        assert!(matches!(DeckFile::from_json("{not json"), Err(DeckError::Invalid(_))));
    }
}
