//! Paging state of one column: which rows are loaded, which cursors to use
//! next, and how the visible list must change after each load.

use std::{collections::HashSet, ops::Range};

use chrono::{DateTime, Utc};
use twitty_model::{BackendError, FeedItem, FeedPage, FeedSource, PageRequest, PageToken};

/// The kind of load a column can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    /// Replace everything with the most recent page.
    Refresh,
    /// Append older rows at the bottom.
    Older,
    /// Prepend newer rows at the top.
    Newer,
}

/// What the column is doing right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Loading(LoadKind),
    /// The last load failed; rows already shown stay visible.
    Failed {
        kind: LoadKind,
        error: BackendError,
    },
}

/// How the rendered list must change to match the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListChange {
    None,
    /// Drop everything and show `count` rows.
    Reset {
        count: usize,
    },
    /// Rows `range` were replaced by `count` new rows.
    Splice {
        range: Range<usize>,
        count: usize,
    },
}

/// Paging state of a column.
#[derive(Debug)]
pub struct ColumnModel {
    source: FeedSource,
    items: Vec<FeedItem>,
    keys: HashSet<String>,
    newer: Option<PageToken>,
    older: Option<PageToken>,
    status: Status,
    loaded_once: bool,
    /// Rows prepended by `Newer` loads that the user has not scrolled up to yet.
    unseen: usize,
    /// Row highlighted for keyboard actions.
    selected: Option<usize>,
}

impl ColumnModel {
    /// An empty column for `source`.
    pub fn new(source: FeedSource) -> Self {
        Self {
            source,
            items: Vec::new(),
            keys: HashSet::new(),
            newer: None,
            older: None,
            status: Status::Idle,
            loaded_once: false,
            unseen: 0,
            selected: None,
        }
    }

    /// The row highlighted for keyboard actions.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Moves the selection by `delta` rows (clamped); from no selection, any
    /// move selects the first row. Returns the new selection.
    pub fn select(&mut self, delta: isize) -> Option<usize> {
        let last = self.items.len().checked_sub(1)?;
        self.selected = Some(match self.selected {
            None => 0,
            Some(ix) => ix.saturating_add_signed(delta).min(last),
        });
        self.selected
    }

    /// The first post of the selected row (a thread's first post, a
    /// notification's target).
    pub fn selected_post(&self) -> Option<&twitty_model::Post> {
        self.items.get(self.selected?)?.posts().into_iter().next()
    }

    pub fn source(&self) -> &FeedSource {
        &self.source
    }

    pub fn items(&self) -> &[FeedItem] {
        &self.items
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Whether the first page has been received.
    pub fn loaded_once(&self) -> bool {
        self.loaded_once
    }

    /// Whether more rows exist below.
    pub fn has_older(&self) -> bool {
        self.older.is_some()
    }

    /// New rows above the viewport the user has not seen.
    pub fn unseen(&self) -> usize {
        self.unseen
    }

    /// The user scrolled to the top: everything is seen.
    pub fn mark_seen(&mut self) {
        self.unseen = 0;
    }

    /// Starts a load if it makes sense at `now`, returning the request to send.
    ///
    /// Returns `None` when another load is in flight, when there is no cursor
    /// for the requested direction, or while the last failure says retrying
    /// is pointless: until a rate limit resets, and after `Unauthorized` for
    /// anything but an explicit `Refresh` (polling and scrolling would only
    /// repeat the error). `Older`/`Newer` before the first page are turned
    /// into a `Refresh`.
    pub fn begin(&mut self, kind: LoadKind, now: DateTime<Utc>) -> Option<(LoadKind, PageRequest)> {
        match &self.status {
            Status::Loading(_) => return None,
            Status::Failed { error: BackendError::RateLimited { reset_at }, .. }
                if now < *reset_at =>
            {
                return None;
            }
            Status::Failed { error: BackendError::Unauthorized, .. }
                if kind != LoadKind::Refresh =>
            {
                return None;
            }
            _ => {}
        }
        let (kind, request) = match kind {
            _ if !self.loaded_once => (LoadKind::Refresh, PageRequest::First),
            LoadKind::Refresh => (LoadKind::Refresh, PageRequest::First),
            LoadKind::Older => (LoadKind::Older, PageRequest::Older(self.older.clone()?)),
            LoadKind::Newer => match &self.newer {
                Some(token) => (LoadKind::Newer, PageRequest::Newer(token.clone())),
                None => (LoadKind::Refresh, PageRequest::First),
            },
        };
        self.status = Status::Loading(kind);
        Some((kind, request))
    }

    /// What a "Retry" after the last failure should load: the failed kind,
    /// except after `Unauthorized`, where only a `Refresh` is attempted.
    /// `None` when nothing failed.
    pub fn retry_kind(&self) -> Option<LoadKind> {
        match &self.status {
            Status::Failed { error: BackendError::Unauthorized, .. } => Some(LoadKind::Refresh),
            Status::Failed { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Applies the result of a load started with [`Self::begin`].
    pub fn finish(&mut self, kind: LoadKind, result: Result<FeedPage, BackendError>) -> ListChange {
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.status = Status::Failed { kind, error };
                return ListChange::None;
            }
        };
        self.status = Status::Idle;
        match kind {
            LoadKind::Refresh => {
                self.keys.clear();
                // Same end-of-feed rule as `Older`: no rows means nothing below.
                self.older = if page.items.is_empty() { None } else { page.older };
                self.items = self.dedup(page.items);
                self.newer = page.newer;
                self.loaded_once = true;
                self.unseen = 0;
                self.selected = None;
                ListChange::Reset { count: self.items.len() }
            }
            LoadKind::Older => {
                // An exhausted feed returns no cursor, or (X) a fresh cursor
                // with no rows; either way stop asking.
                self.older = if page.items.is_empty() { None } else { page.older };
                let fresh = self.dedup(page.items);
                if fresh.is_empty() {
                    return ListChange::None;
                }
                let start = self.items.len();
                let count = fresh.len();
                self.items.extend(fresh);
                ListChange::Splice { range: start..start, count }
            }
            LoadKind::Newer => {
                if page.newer.is_some() {
                    self.newer = page.newer;
                }
                let fresh = self.dedup(page.items);
                if fresh.is_empty() {
                    return ListChange::None;
                }
                let count = fresh.len();
                self.unseen += count;
                // Keep the same row selected when rows arrive above it.
                self.selected = self.selected.map(|ix| ix + count);
                self.items.splice(0..0, fresh);
                ListChange::Splice { range: 0..0, count }
            }
        }
    }

    /// Replaces the source (e.g. switching Home between For you and Following)
    /// and clears all rows.
    pub fn set_source(&mut self, source: FeedSource) -> ListChange {
        *self = Self::new(source);
        ListChange::Reset { count: 0 }
    }

    fn dedup(&mut self, items: Vec<FeedItem>) -> Vec<FeedItem> {
        items.into_iter().filter(|item| self.keys.insert(item.key())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use twitty_model::{Backend, HomeMode, mock::MockBackend};

    fn home() -> FeedSource {
        FeedSource::Home { mode: HomeMode::Following }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    /// Drives one load end to end against the mock.
    fn load(model: &mut ColumnModel, backend: &MockBackend, kind: LoadKind) -> Option<ListChange> {
        let (kind, req) = model.begin(kind, Utc::now())?;
        let source = model.source().clone();
        let result = block_on(backend.feed(&source, req));
        Some(model.finish(kind, result))
    }

    #[test]
    fn first_load_resets_list() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        assert!(!m.loaded_once());
        let change = load(&mut m, &backend, LoadKind::Older).unwrap();
        assert_eq!(change, ListChange::Reset { count: MockBackend::PAGE_SIZE });
        assert!(m.loaded_once());
        assert_eq!(m.status(), &Status::Idle);
    }

    #[test]
    fn begin_refuses_concurrent_loads() {
        let mut m = ColumnModel::new(home());
        assert!(m.begin(LoadKind::Refresh, Utc::now()).is_some());
        assert!(m.begin(LoadKind::Older, Utc::now()).is_none());
        assert!(m.begin(LoadKind::Newer, Utc::now()).is_none());
    }

    #[test]
    fn older_appends_until_exhausted() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        let change = load(&mut m, &backend, LoadKind::Older).unwrap();
        assert_eq!(
            change,
            ListChange::Splice {
                range: MockBackend::PAGE_SIZE..MockBackend::PAGE_SIZE,
                count: MockBackend::PAGE_SIZE
            }
        );
        while m.has_older() {
            load(&mut m, &backend, LoadKind::Older);
        }
        assert_eq!(m.items().len(), MockBackend::FEED_LEN);
        assert!(m.begin(LoadKind::Older, Utc::now()).is_none(), "no cursor once exhausted");
    }

    #[test]
    fn newer_prepends_and_counts_unseen() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        backend.push_new(&home(), 4);
        let change = load(&mut m, &backend, LoadKind::Newer).unwrap();
        assert_eq!(change, ListChange::Splice { range: 0..0, count: 4 });
        assert_eq!(m.unseen(), 4);
        backend.push_new(&home(), 1);
        load(&mut m, &backend, LoadKind::Newer);
        assert_eq!(m.unseen(), 5);
        m.mark_seen();
        assert_eq!(m.unseen(), 0);
    }

    #[test]
    fn newer_with_nothing_new_changes_nothing() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        assert_eq!(load(&mut m, &backend, LoadKind::Newer), Some(ListChange::None));
        assert_eq!(m.items().len(), MockBackend::PAGE_SIZE);
    }

    #[test]
    fn duplicates_across_pages_are_dropped() {
        let mut m = ColumnModel::new(home());
        let backend = MockBackend::new();
        load(&mut m, &backend, LoadKind::Refresh);
        let dup = m.items()[0].clone();
        m.begin(LoadKind::Older, Utc::now());
        let change =
            m.finish(LoadKind::Older, Ok(FeedPage { items: vec![dup], newer: None, older: None }));
        assert_eq!(change, ListChange::None);
        assert_eq!(m.items().len(), MockBackend::PAGE_SIZE);
    }

    #[test]
    fn failure_keeps_rows_and_allows_retry() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        backend.fail_next(BackendError::Network("offline".into()));
        assert_eq!(load(&mut m, &backend, LoadKind::Older), Some(ListChange::None));
        assert!(matches!(m.status(), Status::Failed { kind: LoadKind::Older, .. }));
        assert_eq!(m.items().len(), MockBackend::PAGE_SIZE);
        // Retry works.
        assert!(matches!(load(&mut m, &backend, LoadKind::Older), Some(ListChange::Splice { .. })));
    }

    #[test]
    fn empty_older_page_with_a_cursor_ends_the_feed() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        m.begin(LoadKind::Older, Utc::now());
        let empty = FeedPage { items: vec![], newer: None, older: Some(PageToken::new("again")) };
        assert_eq!(m.finish(LoadKind::Older, Ok(empty)), ListChange::None);
        assert!(!m.has_older(), "no endless requests at the bottom");
    }

    #[test]
    fn empty_first_page_is_the_whole_feed() {
        let mut m = ColumnModel::new(home());
        m.begin(LoadKind::Refresh, Utc::now());
        let empty = FeedPage { items: vec![], newer: None, older: Some(PageToken::new("more")) };
        assert_eq!(m.finish(LoadKind::Refresh, Ok(empty)), ListChange::Reset { count: 0 });
        assert!(m.loaded_once() && !m.has_older());
    }

    #[test]
    fn selection_moves_clamps_and_follows_new_rows() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        assert_eq!(m.select(1), None, "nothing to select yet");
        load(&mut m, &backend, LoadKind::Refresh);
        assert_eq!(m.select(1), Some(0), "first move selects the top row");
        assert_eq!(m.select(1), Some(1));
        assert_eq!(m.select(-5), Some(0));
        assert_eq!(m.select(1000), Some(MockBackend::PAGE_SIZE - 1));
        let selected = m.selected_post().map(|p| p.id.clone());
        backend.push_new(&home(), 2);
        load(&mut m, &backend, LoadKind::Newer);
        assert_eq!(m.selected(), Some(MockBackend::PAGE_SIZE + 1));
        assert_eq!(m.selected_post().map(|p| p.id.clone()), selected, "same post");
        load(&mut m, &backend, LoadKind::Refresh);
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn rate_limit_holds_loads_until_reset() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        let now = Utc::now();
        let reset_at = now + chrono::Duration::minutes(5);
        m.begin(LoadKind::Newer, now);
        m.finish(LoadKind::Newer, Err(BackendError::RateLimited { reset_at }));
        for kind in [LoadKind::Newer, LoadKind::Older, LoadKind::Refresh] {
            assert!(m.begin(kind, now).is_none(), "{kind:?} before reset");
        }
        assert!(m.begin(LoadKind::Newer, reset_at).is_some(), "allowed once reset");
    }

    #[test]
    fn unauthorized_stops_automatic_loads_but_not_refresh() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        backend.fail_next(BackendError::Unauthorized);
        load(&mut m, &backend, LoadKind::Newer);
        assert!(m.begin(LoadKind::Newer, Utc::now()).is_none(), "polling stops");
        assert!(m.begin(LoadKind::Older, Utc::now()).is_none(), "scrolling stops");
        assert_eq!(m.retry_kind(), Some(LoadKind::Refresh));
        assert!(m.begin(LoadKind::Refresh, Utc::now()).is_some(), "the user can retry");
    }

    #[test]
    fn retry_repeats_other_failed_loads() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        assert_eq!(m.retry_kind(), None);
        load(&mut m, &backend, LoadKind::Refresh);
        backend.fail_next(BackendError::Network("offline".into()));
        load(&mut m, &backend, LoadKind::Older);
        assert_eq!(m.retry_kind(), Some(LoadKind::Older));
    }

    #[test]
    fn refresh_replaces_rows_and_clears_unseen() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        load(&mut m, &backend, LoadKind::Older);
        backend.push_new(&home(), 2);
        load(&mut m, &backend, LoadKind::Newer);
        let change = load(&mut m, &backend, LoadKind::Refresh).unwrap();
        assert_eq!(change, ListChange::Reset { count: MockBackend::PAGE_SIZE });
        assert_eq!(m.unseen(), 0);
    }

    #[test]
    fn set_source_clears_state() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        let other = FeedSource::Home { mode: HomeMode::ForYou };
        assert_eq!(m.set_source(other.clone()), ListChange::Reset { count: 0 });
        assert_eq!(m.source(), &other);
        assert!(m.items().is_empty());
        assert!(!m.loaded_once());
    }
}
