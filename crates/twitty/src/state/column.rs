//! Paging state of one column: which rows are loaded, which of them the
//! column's filters show, which cursors to use next, and how the visible list
//! must change after each load.

use std::{collections::HashSet, ops::Range};

use chrono::{DateTime, Utc};
use twitty_model::{BackendError, FeedItem, FeedPage, FeedSource, PageRequest, PageToken};

use crate::state::filters::ColumnFilters;

/// How many pages in a row may come back with every row hidden by the
/// filters before the column stops loading more by itself (scrolling still
/// loads more). Keeps a strict filter from walking the whole feed.
pub const MAX_HIDDEN_PAGES: u8 = 3;

/// The kind of load a column can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    /// Replace everything with the most recent page.
    Refresh,
    /// Append older rows at the bottom.
    Older,
    /// Prepend newer rows at the top, below any pinned rows.
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
///
/// Every index it hands out (rows, [`ListChange`], selection, unseen rows)
/// counts the rows the filters show.
#[derive(Debug)]
pub struct ColumnModel {
    source: FeedSource,
    filters: ColumnFilters,
    /// Every row loaded, shown or not, in order.
    all: Vec<FeedItem>,
    /// The rows of `all` the filters show.
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
    /// Pages in a row (refreshed or older) whose rows were all hidden.
    hidden_pages: u8,
}

impl ColumnModel {
    /// An empty column for `source`.
    pub fn new(source: FeedSource) -> Self {
        Self {
            source,
            filters: ColumnFilters::default(),
            all: Vec::new(),
            items: Vec::new(),
            keys: HashSet::new(),
            newer: None,
            older: None,
            status: Status::Idle,
            loaded_once: false,
            unseen: 0,
            selected: None,
            hidden_pages: 0,
        }
    }

    /// An empty column for `source` that shows what `filters` let through.
    pub fn with_filters(source: FeedSource, filters: ColumnFilters) -> Self {
        Self { filters, ..Self::new(source) }
    }

    pub fn filters(&self) -> &ColumnFilters {
        &self.filters
    }

    /// Changes which rows show. The loaded rows are filtered again, so the
    /// list starts over (nothing selected or unseen).
    pub fn set_filters(&mut self, filters: ColumnFilters) -> ListChange {
        if filters == self.filters {
            return ListChange::None;
        }
        self.filters = filters;
        self.items = self.shown(&self.all);
        self.selected = None;
        self.unseen = 0;
        self.hidden_pages = 0;
        ListChange::Reset { count: self.items.len() }
    }

    /// Whether every loaded row is hidden by the filters (there are some).
    pub fn all_hidden(&self) -> bool {
        self.items.is_empty() && !self.all.is_empty()
    }

    /// Whether the last loads brought rows the filters all hid, so the column
    /// should load older ones by itself (at most [`MAX_HIDDEN_PAGES`] times in
    /// a row) instead of waiting for a scroll that may never come.
    pub fn wants_more(&self) -> bool {
        self.has_older() && (1..=MAX_HIDDEN_PAGES).contains(&self.hidden_pages)
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

    /// Drops the selection; the next move selects the first row again.
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// The first tweet of the selected row (a thread's first tweet, a
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

    /// Where those unseen rows are: right below the pinned rows.
    pub fn unseen_rows(&self) -> Range<usize> {
        let start = self.pinned();
        start..start + self.unseen
    }

    /// How many rows at the top are pinned tweets. Newer rows go below them.
    pub fn pinned(&self) -> usize {
        leading_pins(&self.items)
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
                self.all = self.dedup(page.items);
                self.items = self.shown(&self.all);
                self.count_hidden_page(self.all.len(), self.items.len());
                self.newer = page.newer;
                self.loaded_once = true;
                self.unseen = 0;
                self.selected = None;
                ListChange::Reset { count: self.items.len() }
            }
            LoadKind::Older => {
                // An exhausted feed returns no cursor, or (Twitter) a fresh cursor
                // with no rows; either way stop asking.
                self.older = if page.items.is_empty() { None } else { page.older };
                let fresh = self.dedup(unpinned(page.items));
                let shown = self.shown(&fresh);
                self.count_hidden_page(fresh.len(), shown.len());
                self.all.extend(fresh);
                if shown.is_empty() {
                    return ListChange::None;
                }
                let start = self.items.len();
                let count = shown.len();
                self.items.extend(shown);
                ListChange::Splice { range: start..start, count }
            }
            LoadKind::Newer => {
                if page.newer.is_some() {
                    self.newer = page.newer;
                }
                let fresh = self.dedup(unpinned(page.items));
                let shown = self.shown(&fresh);
                // Hidden pins still lead `all`, so its insertion point can
                // differ from the visible one.
                let at_all = leading_pins(&self.all);
                self.all.splice(at_all..at_all, fresh);
                if shown.is_empty() {
                    return ListChange::None;
                }
                let count = shown.len();
                let at = self.pinned();
                self.unseen += count;
                // Keep the same row selected when rows arrive above it.
                self.selected = self.selected.map(|ix| if ix >= at { ix + count } else { ix });
                self.items.splice(at..at, shown);
                ListChange::Splice { range: at..at, count }
            }
        }
    }

    /// Replaces the source (e.g. switching Home between For you and Following)
    /// and clears all rows.
    pub fn set_source(&mut self, source: FeedSource) -> ListChange {
        *self = Self::with_filters(source, std::mem::take(&mut self.filters));
        ListChange::Reset { count: 0 }
    }

    fn dedup(&mut self, items: Vec<FeedItem>) -> Vec<FeedItem> {
        items.into_iter().filter(|item| self.keys.insert(item.key())).collect()
    }

    /// The rows of `items` the filters show.
    fn shown(&self, items: &[FeedItem]) -> Vec<FeedItem> {
        items.iter().filter(|item| self.filters.shows(item)).cloned().collect()
    }

    /// Counts a page of `loaded` new rows, of which `shown` show.
    fn count_hidden_page(&mut self, loaded: usize, shown: usize) {
        self.hidden_pages =
            if loaded > 0 && shown == 0 { self.hidden_pages.saturating_add(1) } else { 0 };
    }
}

/// How many rows at the top of `items` are pinned tweets.
fn leading_pins(items: &[FeedItem]) -> usize {
    items.iter().take_while(|item| matches!(item, FeedItem::Pinned(_))).count()
}

/// `items` without pinned tweets. Pins are only taken from a refresh, so they
/// stay at the top: a pin seen while paging (a new pin, or Twitter sending it
/// again) waits for the next refresh.
fn unpinned(items: Vec<FeedItem>) -> Vec<FeedItem> {
    items.into_iter().filter(|item| !matches!(item, FeedItem::Pinned(_))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use twitty_model::{Backend, HomeMode, PostId, mock::MockBackend};

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
        assert_eq!(m.selected_post().map(|p| p.id.clone()), selected, "same tweet");
        load(&mut m, &backend, LoadKind::Refresh);
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn clearing_the_selection_starts_again_from_the_top() {
        let backend = MockBackend::new();
        let mut m = ColumnModel::new(home());
        load(&mut m, &backend, LoadKind::Refresh);
        m.select(1);
        m.select(1);
        m.clear_selection();
        assert_eq!(m.selected(), None);
        assert_eq!(m.select(1), Some(0));
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

    fn pinned_home(backend: &MockBackend) -> (ColumnModel, PostId) {
        let pin = backend.pin_new(&home());
        let mut m = ColumnModel::new(home());
        load(&mut m, backend, LoadKind::Refresh);
        (m, pin)
    }

    fn is_pin(item: &FeedItem, pin: &PostId) -> bool {
        matches!(item, FeedItem::Pinned(p) if &p.id == pin)
    }

    #[test]
    fn newer_rows_go_below_the_pin() {
        let backend = MockBackend::new();
        let (mut m, pin) = pinned_home(&backend);
        assert_eq!(m.pinned(), 1);
        assert!(is_pin(&m.items()[0], &pin));
        let pushed = backend.push_new(&home(), 2);
        let change = load(&mut m, &backend, LoadKind::Newer).unwrap();
        assert_eq!(change, ListChange::Splice { range: 1..1, count: 2 });
        assert!(is_pin(&m.items()[0], &pin), "the pin stays on top");
        let newest: Vec<_> =
            m.items()[1..3].iter().flat_map(|i| i.posts()).map(|p| &p.id).collect();
        assert_eq!(newest, [&pushed[1], &pushed[0]]);
        assert_eq!(m.unseen_rows(), 1..3);
    }

    #[test]
    fn a_selected_pin_stays_selected_when_newer_rows_arrive() {
        let backend = MockBackend::new();
        let (mut m, pin) = pinned_home(&backend);
        assert_eq!(m.select(1), Some(0));
        backend.push_new(&home(), 2);
        load(&mut m, &backend, LoadKind::Newer);
        assert_eq!(m.selected(), Some(0));
        assert_eq!(m.selected_post().map(|p| p.id.clone()), Some(pin));
        // A row below the pin moves down with it.
        m.select(1);
        let below = m.selected_post().map(|p| p.id.clone());
        backend.push_new(&home(), 1);
        load(&mut m, &backend, LoadKind::Newer);
        assert_eq!(m.selected(), Some(2));
        assert_eq!(m.selected_post().map(|p| p.id.clone()), below);
    }

    #[test]
    fn pins_on_later_pages_wait_for_a_refresh() {
        let backend = MockBackend::new();
        let (mut m, pin) = pinned_home(&backend);
        let other = backend.pin_new(&home());
        let stray = |id: &PostId| {
            let post = m.items()[1].posts()[0].clone();
            FeedItem::Pinned(twitty_model::Post { id: id.clone(), ..post })
        };
        let (old, new) = (stray(&pin), stray(&other));
        for kind in [LoadKind::Newer, LoadKind::Older] {
            m.begin(kind, Utc::now());
            let page = FeedPage { items: vec![old.clone(), new.clone()], newer: None, older: None };
            assert_eq!(m.finish(kind, Ok(page)), ListChange::None, "{kind:?}");
        }
        assert_eq!(m.pinned(), 1);
        load(&mut m, &backend, LoadKind::Refresh);
        assert_eq!(m.pinned(), 1);
        assert!(is_pin(&m.items()[0], &other), "the refresh brings the new pin");
    }

    #[test]
    fn a_pin_keeps_its_timeline_row() {
        let backend = MockBackend::new();
        let (mut m, pin) = pinned_home(&backend);
        let post = m.items()[0].posts()[0].clone();
        m.begin(LoadKind::Older, Utc::now());
        let page = FeedPage { items: vec![FeedItem::Post(post)], newer: None, older: None };
        let change = m.finish(LoadKind::Older, Ok(page));
        assert!(
            matches!(change, ListChange::Splice { count: 1, .. }),
            "not a duplicate of the pin"
        );
        assert!(matches!(m.items().last(), Some(FeedItem::Post(p)) if p.id == pin));
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

    use crate::state::filters::{TweetKind, kinds};
    use twitty_model::{Media, Post, ReplyRef};

    /// A text-only tweet with id `id`.
    fn text(id: &str) -> Post {
        let backend = MockBackend::new();
        let page = block_on(backend.feed(&home(), PageRequest::First)).unwrap();
        let post = page.items[0].posts()[0].clone();
        Post { id: PostId::new(id), media: vec![], embed: None, reply_to: None, ..post }
    }

    /// A tweet with a picture, with id `id`.
    fn picture(id: &str) -> Post {
        let mut post = text(id);
        let image = twitty_model::ImageRef { url: post.url.clone(), size: None, crops: vec![] };
        post.media.push(Media::Image { image, alt: None });
        post
    }

    /// Feeds `items` to a load of `kind`, with cursors both ways.
    fn feed(m: &mut ColumnModel, kind: LoadKind, items: Vec<FeedItem>) -> ListChange {
        m.begin(kind, Utc::now()).expect("a load can start");
        let page = FeedPage {
            items,
            newer: Some(PageToken::new("newer")),
            older: Some(PageToken::new("older")),
        };
        m.finish(kind, Ok(page))
    }

    fn ids(items: &[FeedItem]) -> Vec<String> {
        items.iter().flat_map(|i| i.posts()).map(|p| p.id.to_string()).collect()
    }

    #[test]
    fn filters_hide_rows_until_changed_back() {
        let backend = MockBackend::new();
        let hidden = [TweetKind::Retweets, TweetKind::Videos];
        let mut m = ColumnModel::with_filters(home(), ColumnFilters::hiding(&hidden));
        let change = load(&mut m, &backend, LoadKind::Refresh).unwrap();
        let shown = m.items().len();
        assert!(shown < MockBackend::PAGE_SIZE, "the mock's first page has a retweet");
        assert_eq!(change, ListChange::Reset { count: shown });
        assert!(m.items().iter().all(|i| hidden.iter().all(|k| !kinds(i).contains(k))));

        m.select(1);
        assert_eq!(m.set_filters(ColumnFilters::hiding(&hidden)), ListChange::None, "no change");
        let change = m.set_filters(ColumnFilters::default());
        assert_eq!(change, ListChange::Reset { count: MockBackend::PAGE_SIZE });
        assert_eq!(m.items().len(), MockBackend::PAGE_SIZE, "hidden rows come back");
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn hidden_new_rows_are_neither_unseen_nor_new_again() {
        let mut m = ColumnModel::with_filters(home(), ColumnFilters::hiding(&[TweetKind::Images]));
        feed(&mut m, LoadKind::Refresh, vec![FeedItem::Post(text("1"))]);
        let change = feed(
            &mut m,
            LoadKind::Newer,
            vec![FeedItem::Post(picture("3")), FeedItem::Post(text("2"))],
        );
        assert_eq!(change, ListChange::Splice { range: 0..0, count: 1 });
        assert_eq!(m.unseen(), 1, "only the shown row counts");
        assert_eq!(ids(m.items()), ["2", "1"]);

        let again = feed(&mut m, LoadKind::Newer, vec![FeedItem::Post(picture("3"))]);
        assert_eq!(again, ListChange::None, "a hidden row is still known");
        let only_hidden = feed(&mut m, LoadKind::Newer, vec![FeedItem::Post(picture("4"))]);
        assert_eq!(only_hidden, ListChange::None);
        assert_eq!(m.unseen(), 1);

        m.set_filters(ColumnFilters::default());
        assert_eq!(ids(m.items()), ["4", "3", "2", "1"], "in feed order");
        assert_eq!(m.unseen(), 0);
    }

    #[test]
    fn newer_rows_keep_their_place_below_a_hidden_pin() {
        let mut m =
            ColumnModel::with_filters(home(), ColumnFilters::hiding(&[TweetKind::TextOnly]));
        feed(
            &mut m,
            LoadKind::Refresh,
            vec![FeedItem::Pinned(text("p")), FeedItem::Post(picture("1"))],
        );
        assert_eq!(m.pinned(), 0, "the pin is hidden");
        let change = feed(&mut m, LoadKind::Newer, vec![FeedItem::Post(picture("2"))]);
        assert_eq!(change, ListChange::Splice { range: 0..0, count: 1 });
        m.set_filters(ColumnFilters::default());
        assert_eq!(ids(m.items()), ["p", "2", "1"], "the pin still leads");
        assert_eq!(m.pinned(), 1);
    }

    #[test]
    fn pages_hidden_whole_ask_for_more_a_few_times() {
        let mut m = ColumnModel::with_filters(home(), ColumnFilters::hiding(&[TweetKind::Images]));
        assert!(!m.wants_more());
        feed(&mut m, LoadKind::Refresh, vec![FeedItem::Post(picture("1"))]);
        assert!(m.all_hidden());
        for n in 2..=MAX_HIDDEN_PAGES + 1 {
            assert!(m.wants_more(), "after {} hidden pages", n - 1);
            feed(&mut m, LoadKind::Older, vec![FeedItem::Post(picture(&n.to_string()))]);
        }
        assert!(!m.wants_more(), "gives up until the user scrolls");

        let change = feed(&mut m, LoadKind::Older, vec![FeedItem::Post(text("9"))]);
        assert_eq!(change, ListChange::Splice { range: 0..0, count: 1 });
        assert!(!m.wants_more() && !m.all_hidden());
        feed(&mut m, LoadKind::Older, vec![FeedItem::Post(picture("10"))]);
        assert!(m.wants_more(), "the count starts again after a shown row");
    }

    #[test]
    fn a_thread_and_a_retweet_are_filtered_as_a_whole() {
        let mut m = ColumnModel::with_filters(home(), ColumnFilters::hiding(&[TweetKind::Replies]));
        let reply =
            Post { reply_to: Some(ReplyRef { post: None, user: None, handle: None }), ..text("2") };
        let by = std::sync::Arc::clone(&text("x").author);
        feed(
            &mut m,
            LoadKind::Refresh,
            vec![
                FeedItem::Thread(vec![text("1"), reply.clone()]),
                FeedItem::Repost { by, post: reply },
                FeedItem::Post(text("3")),
            ],
        );
        assert_eq!(ids(m.items()), ["3"]);
    }

    #[test]
    fn switching_the_source_keeps_the_filters() {
        let filters = ColumnFilters::hiding(&[TweetKind::Quotes]);
        let mut m = ColumnModel::with_filters(home(), filters.clone());
        m.set_source(FeedSource::Bookmarks);
        assert_eq!(m.filters(), &filters);
    }
}
