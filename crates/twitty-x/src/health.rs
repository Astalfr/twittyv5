//! Per-operation health counters behind `Backend::health`: how many responses
//! translated cleanly, how many rows Twitter sent that Twitty couldn't read, and the
//! last problem seen. This is the early warning for Twitter changing its JSON.

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

use twitty_model::{FeedItem, RequestHealth};

/// Thread-safe counters keyed by operation name.
#[derive(Debug, Default)]
pub(crate) struct Health {
    by_name: Mutex<BTreeMap<String, RequestHealth>>,
}

impl Health {
    fn update(&self, name: &str, f: impl FnOnce(&mut RequestHealth)) {
        let mut map = self.by_name.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = map
            .entry(name.to_owned())
            .or_insert_with(|| RequestHealth { name: name.to_owned(), ..RequestHealth::default() });
        f(entry);
    }

    /// A successful response that carried `items`, with translation `warnings`.
    pub(crate) fn page(&self, name: &str, items: &[FeedItem], warnings: &[String]) {
        let unsupported =
            items.iter().filter(|i| matches!(i, FeedItem::Unsupported { .. })).count();
        self.rows(name, items.len(), unsupported, warnings);
    }

    /// A successful response with `rows` rows, `unsupported` of them
    /// unreadable, and translation `warnings`.
    pub(crate) fn rows(&self, name: &str, rows: usize, unsupported: usize, warnings: &[String]) {
        self.update(name, |h| {
            h.ok += 1;
            h.rows += rows as u64;
            h.unsupported += unsupported as u64;
            h.warnings += warnings.len() as u64;
            if let Some(last) = warnings.last() {
                h.last_warning = Some(last.clone());
            }
        });
    }

    /// A successful response without rows (a profile, a mutation…).
    pub(crate) fn ok(&self, name: &str) {
        self.update(name, |h| h.ok += 1);
    }

    /// A response that could not be translated at all.
    pub(crate) fn warning(&self, name: &str, warning: String) {
        self.update(name, |h| {
            h.warnings += 1;
            h.last_warning = Some(warning);
        });
    }

    /// A failed request. `error` must not contain secrets (`XError` never does).
    pub(crate) fn error(&self, name: &str, error: &dyn std::fmt::Display) {
        let text = error.to_string();
        self.update(name, |h| {
            h.errors += 1;
            h.last_error = Some(text);
        });
    }

    /// Everything, sorted by name.
    pub(crate) fn snapshot(&self) -> Vec<RequestHealth> {
        self.by_name.lock().unwrap_or_else(PoisonError::into_inner).values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_accumulate_per_name() {
        let h = Health::default();
        let unsupported = FeedItem::Unsupported { entry: "e".into(), reason: "new type".into() };
        h.page("HomeTimeline", std::slice::from_ref(&unsupported), &["e: new type".into()]);
        h.page("HomeTimeline", &[], &[]);
        h.error("HomeTimeline", &"network error: timeout");
        h.ok("FavoriteTweet");
        let snap = h.snapshot();
        let names: Vec<_> = snap.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["FavoriteTweet", "HomeTimeline"], "sorted");
        let home = &snap[1];
        assert_eq!(
            (home.ok, home.rows, home.unsupported, home.warnings, home.errors),
            (2, 1, 1, 1, 1)
        );
        assert_eq!(home.last_warning.as_deref(), Some("e: new type"));
        assert_eq!(home.last_error.as_deref(), Some("network error: timeout"));
    }
}
