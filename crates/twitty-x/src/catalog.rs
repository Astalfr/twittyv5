//! Where GraphQL query ids come from, and the `features` sent with them.
//!
//! Resolution order for an operation: explicit override (CLI / tests) →
//! runtime cache (refreshed from x.com's bundles, saved on disk) → embedded
//! fallback (`data/operations.json`). See `docs/x-api.md`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{PoisonError, RwLock},
};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const EMBEDDED_OPERATIONS: &str = include_str!("../data/operations.json");
const DEFAULT_FEATURES: &str = include_str!("../data/default_features.json");
const CACHE_SCHEMA_VERSION: u32 = 1;
/// How long discovered ids are considered fresh.
pub const CACHE_TTL: Duration = Duration::hours(24);

/// HTTP method of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    /// Queries: variables and features in the URL.
    #[default]
    Get,
    /// Mutations: JSON body.
    Post,
}

/// What we know about one GraphQL operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSpec {
    pub query_id: String,
    #[serde(default)]
    pub method: Method,
    /// Feature flag names the operation declares; `None` = unknown (send all defaults).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_switches: Option<Vec<String>>,
    /// Field toggle names the operation declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_toggles: Option<Vec<String>>,
    /// Free-form provenance, for diagnostics.
    #[serde(default)]
    pub source: String,
}

/// Which layer an operation was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Override,
    Cache,
    Embedded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    schema_version: u32,
    fetched_at: DateTime<Utc>,
    operations: HashMap<String, OperationSpec>,
}

#[derive(Deserialize)]
struct EmbeddedFile {
    operations: HashMap<String, OperationSpec>,
}

/// Thread-safe operation catalog.
#[derive(Debug)]
pub struct Catalog {
    embedded: HashMap<String, OperationSpec>,
    cache: RwLock<Option<CacheFile>>,
    overrides: RwLock<HashMap<String, String>>,
    cache_path: Option<PathBuf>,
    default_features: Map<String, Value>,
}

impl Catalog {
    /// A catalog backed by `cache_path` (loaded now if it exists), or memory only.
    pub fn new(cache_path: Option<PathBuf>) -> Self {
        let embedded = match serde_json::from_str::<EmbeddedFile>(EMBEDDED_OPERATIONS) {
            Ok(f) => f.operations,
            Err(e) => {
                tracing::error!("embedded operations.json is invalid: {e}");
                HashMap::new()
            }
        };
        let default_features = match serde_json::from_str::<Value>(DEFAULT_FEATURES) {
            Ok(Value::Object(m)) => m,
            other => {
                tracing::error!("embedded default_features.json is invalid: {other:?}");
                Map::new()
            }
        };
        let cache = cache_path.as_deref().and_then(load_cache);
        Self {
            embedded,
            cache: RwLock::new(cache),
            overrides: RwLock::new(HashMap::new()),
            cache_path,
            default_features,
        }
    }

    /// Name of the runtime cache file in a cache directory.
    pub const CACHE_FILE: &str = "query-ids.json";

    /// Default cache directory: `%LOCALAPPDATA%\Twitty\cache` on Windows.
    pub fn default_cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Twitty").map(|d| d.cache_dir().to_path_buf())
    }

    /// Default cache location: `%LOCALAPPDATA%\Twitty\cache\query-ids.json` on Windows.
    pub fn default_cache_path() -> Option<PathBuf> {
        Self::default_cache_dir().map(|d| d.join(Self::CACHE_FILE))
    }

    /// Names of the operations in the embedded catalog (the ones Twitty uses).
    pub fn embedded_names(&self) -> std::collections::HashSet<String> {
        self.embedded.keys().cloned().collect()
    }

    /// Forces `operation` to use `query_id` (keeps method/features from lower layers).
    pub fn set_override(&self, operation: &str, query_id: &str) {
        self.overrides
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(operation.to_owned(), query_id.to_owned());
    }

    /// The spec to use for `operation`.
    pub fn resolve(&self, operation: &str) -> Option<(OperationSpec, Origin)> {
        let lower = self.resolve_without_override(operation);
        let overridden =
            self.overrides.read().unwrap_or_else(PoisonError::into_inner).get(operation).cloned();
        match (overridden, lower) {
            (Some(query_id), Some((spec, _))) => Some((
                OperationSpec { query_id, source: "override".into(), ..spec },
                Origin::Override,
            )),
            (Some(query_id), None) => Some((
                OperationSpec {
                    query_id,
                    method: Method::Get,
                    feature_switches: None,
                    field_toggles: None,
                    source: "override".into(),
                },
                Origin::Override,
            )),
            (None, lower) => lower,
        }
    }

    fn resolve_without_override(&self, operation: &str) -> Option<(OperationSpec, Origin)> {
        let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
        let cached = cache.as_ref().and_then(|c| {
            // Relay persisted queries are named `<Operation>Query` in the new web client.
            c.operations
                .get(operation)
                .or_else(|| c.operations.get(&format!("{operation}Query")))
                .cloned()
        });
        drop(cache);
        match cached {
            Some(spec) => Some((spec, Origin::Cache)),
            None => self.embedded.get(operation).cloned().map(|s| (s, Origin::Embedded)),
        }
    }

    /// Whether discovered ids are younger than [`CACHE_TTL`].
    pub fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|c| now - c.fetched_at < CACHE_TTL)
    }

    /// Merges freshly discovered operations into the cache and saves it.
    /// Returns how many operations changed id.
    pub fn merge_discovered(
        &self,
        discovered: HashMap<String, OperationSpec>,
        now: DateTime<Utc>,
    ) -> std::io::Result<usize> {
        let snapshot = {
            let mut guard = self.cache.write().unwrap_or_else(PoisonError::into_inner);
            let cache = guard.get_or_insert_with(|| CacheFile {
                schema_version: CACHE_SCHEMA_VERSION,
                fetched_at: now,
                operations: HashMap::new(),
            });
            let mut changed = 0;
            for (name, spec) in discovered {
                if cache.operations.get(&name).map(|s| &s.query_id) != Some(&spec.query_id) {
                    changed += 1;
                }
                cache.operations.insert(name, spec);
            }
            cache.fetched_at = now;
            (cache.clone(), changed)
        };
        if let Some(path) = &self.cache_path {
            save_cache(path, &snapshot.0)?;
        }
        Ok(snapshot.1)
    }

    /// The `features` query parameter for `spec`.
    pub fn features_for(&self, spec: &OperationSpec) -> Value {
        match &spec.feature_switches {
            Some(names) if !names.is_empty() => Value::Object(
                names
                    .iter()
                    .map(|n| {
                        let v = self.default_features.get(n).cloned().unwrap_or(Value::Bool(true));
                        (n.clone(), v)
                    })
                    .collect(),
            ),
            _ => Value::Object(self.default_features.clone()),
        }
    }

    /// The `fieldToggles` query parameter for `spec`, if it declares any.
    pub fn field_toggles_for(&self, spec: &OperationSpec) -> Option<Value> {
        spec.field_toggles
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| Value::Object(t.iter().map(|n| (n.clone(), Value::Bool(true))).collect()))
    }

    /// Every known operation name with its resolved spec, sorted by name.
    pub fn list(&self) -> Vec<(String, OperationSpec, Origin)> {
        let mut names: Vec<String> = self.embedded.keys().cloned().collect();
        if let Some(c) = self.cache.read().unwrap_or_else(PoisonError::into_inner).as_ref() {
            names.extend(c.operations.keys().cloned());
        }
        names.extend(self.overrides.read().unwrap_or_else(PoisonError::into_inner).keys().cloned());
        names.sort();
        names.dedup();
        names.into_iter().filter_map(|n| self.resolve(&n).map(|(s, o)| (n, s, o))).collect()
    }
}

fn load_cache(path: &Path) -> Option<CacheFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<CacheFile>(&raw) {
        Ok(c) if c.schema_version == CACHE_SCHEMA_VERSION => Some(c),
        Ok(c) => {
            tracing::warn!("ignoring query-id cache with schema {}", c.schema_version);
            None
        }
        Err(e) => {
            tracing::warn!("ignoring unreadable query-id cache {}: {e}", path.display());
            None
        }
    }
}

fn save_cache(path: &Path, cache: &CacheFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cache)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> OperationSpec {
        OperationSpec {
            query_id: id.into(),
            method: Method::Get,
            feature_switches: None,
            field_toggles: None,
            source: "test".into(),
        }
    }

    #[test]
    fn embedded_catalog_has_home_timeline() {
        let c = Catalog::new(None);
        let (s, origin) = c.resolve("HomeTimeline").unwrap();
        assert_eq!(s.query_id, "0toR0MxC06yY3Fc57GXBpA");
        assert_eq!(origin, Origin::Embedded);
        assert_eq!(c.resolve("FavoriteTweet").unwrap().0.method, Method::Post);
    }

    #[test]
    fn cache_beats_embedded_and_override_beats_cache() {
        let c = Catalog::new(None);
        c.merge_discovered(HashMap::from([("HomeTimeline".into(), spec("cached"))]), Utc::now())
            .unwrap();
        assert_eq!(c.resolve("HomeTimeline").unwrap(), (spec("cached"), Origin::Cache));
        c.set_override("HomeTimeline", "forced");
        let (s, o) = c.resolve("HomeTimeline").unwrap();
        assert_eq!((s.query_id.as_str(), o), ("forced", Origin::Override));
    }

    #[test]
    fn relay_names_with_query_suffix_resolve() {
        let c = Catalog::new(None);
        c.merge_discovered(
            HashMap::from([("HomeLatestTimelineQuery".into(), spec("relay"))]),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(c.resolve("HomeLatestTimeline").unwrap().0.query_id, "relay");
    }

    #[test]
    fn unknown_operation_resolves_to_none_unless_overridden() {
        let c = Catalog::new(None);
        assert!(c.resolve("NoSuchOp").is_none());
        c.set_override("NoSuchOp", "x1");
        assert_eq!(c.resolve("NoSuchOp").unwrap().0.method, Method::Get);
    }

    #[test]
    fn merge_counts_changes_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.json");
        let c = Catalog::new(Some(path.clone()));
        let now = Utc::now();
        let ops = HashMap::from([("A".into(), spec("1")), ("B".into(), spec("2"))]);
        assert_eq!(c.merge_discovered(ops.clone(), now).unwrap(), 2);
        assert_eq!(c.merge_discovered(ops, now).unwrap(), 0, "same ids are not changes");
        let reloaded = Catalog::new(Some(path));
        assert_eq!(reloaded.resolve("A").unwrap(), (spec("1"), Origin::Cache));
        assert!(reloaded.is_fresh(now));
        assert!(!reloaded.is_fresh(now + CACHE_TTL + Duration::seconds(1)));
    }

    #[test]
    fn corrupt_cache_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.json");
        std::fs::write(&path, "{not json").unwrap();
        let c = Catalog::new(Some(path));
        assert_eq!(c.resolve("HomeTimeline").unwrap().1, Origin::Embedded);
    }

    #[test]
    fn features_use_declared_switches_with_defaults() {
        let c = Catalog::new(None);
        let mut s = spec("x");
        assert!(c.features_for(&s).as_object().unwrap().len() > 100, "unknown → all defaults");
        s.feature_switches = Some(vec![
            "verified_phone_label_enabled".into(),
            "brand_new_flag_we_do_not_know".into(),
        ]);
        let f = c.features_for(&s);
        assert_eq!(f["verified_phone_label_enabled"], Value::Bool(false));
        assert_eq!(f["brand_new_flag_we_do_not_know"], Value::Bool(true));
        assert_eq!(f.as_object().unwrap().len(), 2);
    }

    #[test]
    fn field_toggles_only_when_declared() {
        let c = Catalog::new(None);
        let mut s = spec("x");
        assert!(c.field_toggles_for(&s).is_none());
        s.field_toggles = Some(vec!["withArticleRichContentState".into()]);
        assert_eq!(c.field_toggles_for(&s).unwrap()["withArticleRichContentState"], true);
    }
}
