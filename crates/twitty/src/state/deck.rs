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
pub const SCHEMA_VERSION: u32 = 1;

/// Stable identifier of a column within a deck file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnId(pub u64);

/// Column width preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnWidth {
    Narrow,
    #[default]
    Medium,
    Wide,
}

impl ColumnWidth {
    /// Width in logical pixels.
    pub fn px(self) -> f32 {
        match self {
            ColumnWidth::Narrow => 310.0,
            ColumnWidth::Medium => 350.0,
            ColumnWidth::Wide => 400.0,
        }
    }

    /// The next preset, wrapping around.
    pub fn cycle(self) -> Self {
        match self {
            ColumnWidth::Narrow => ColumnWidth::Medium,
            ColumnWidth::Medium => ColumnWidth::Wide,
            ColumnWidth::Wide => ColumnWidth::Narrow,
        }
    }
}

/// One column of a deck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnConfig {
    pub id: ColumnId,
    pub source: FeedSource,
    #[serde(default)]
    pub width: ColumnWidth,
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
        // Migrations from older versions go here, one step at a time.
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
        self.active_mut().columns.push(ColumnConfig { id, source, width: ColumnWidth::default() });
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
        assert_eq!(cols[1].width, ColumnWidth::Wide);
        // Width missing in the file falls back to the default.
        assert_eq!(cols[2].width, ColumnWidth::Medium);
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
    fn width_cycles_through_presets() {
        let w = ColumnWidth::Narrow;
        assert_eq!(w.cycle().cycle().cycle(), w);
        assert!(ColumnWidth::Narrow.px() < ColumnWidth::Wide.px());
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
