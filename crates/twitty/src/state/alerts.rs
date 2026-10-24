//! What a column announces (sound, system notification) when new rows
//! arrive while it polls.

use twitty_model::FeedItem;

use crate::state::column::{ListChange, LoadKind};

/// Longest preview in a notification, in characters (an ellipsis follows).
pub const PREVIEW_CHARS: usize = 120;

/// One batch of new rows, as the system notification shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// How many rows arrived.
    pub count: usize,
    /// The notification's text: what arrived, then the newest row.
    pub body: String,
}

/// The announcement for the rows a load of `kind` added, if it is one to
/// announce: only newer rows prepended by polling (or a manual check). The
/// first page, a refresh and older pages are not news.
///
/// `items` are the column's rows after the change.
pub fn announce(kind: LoadKind, change: &ListChange, items: &[FeedItem]) -> Option<Announcement> {
    let count = match (kind, change) {
        (LoadKind::Newer, ListChange::Splice { range, count }) if range.start == 0 => *count,
        _ => return None,
    };
    let fresh = items.get(..count)?;
    let news: Vec<&FeedItem> = fresh.iter().filter(|item| is_news(item)).collect();
    let newest = news.first()?;
    let preview = preview(newest);
    let body = match news.len() {
        1 => preview,
        n => {
            let noun = if news.iter().all(|i| matches!(i, FeedItem::Notification(_))) {
                "notifications"
            } else {
                "posts"
            };
            format!("{n} new {noun}\n{preview}")
        }
    };
    Some(Announcement { count: news.len(), body })
}

/// Rows worth telling the user about (not tombstones or unreadable rows).
fn is_news(item: &FeedItem) -> bool {
    matches!(
        item,
        FeedItem::Post(_)
            | FeedItem::Repost { .. }
            | FeedItem::Thread(_)
            | FeedItem::Notification(_)
    )
}

/// One line about `item`: who, and the start of what they wrote.
fn preview(item: &FeedItem) -> String {
    let (who, text) = match item {
        FeedItem::Post(post) => (post.author.display_name.clone(), post.body.text()),
        FeedItem::Repost { by, post } => {
            (format!("{} reposted {}", by.display_name, post.author.display_name), post.body.text())
        }
        FeedItem::Thread(posts) => match posts.last() {
            Some(post) => (post.author.display_name.clone(), post.body.text()),
            None => return String::new(),
        },
        // The summary line already names who did what.
        FeedItem::Notification(n) => return shorten(n.text.text()),
        FeedItem::Unavailable { .. } | FeedItem::Unsupported { .. } => return String::new(),
    };
    let text = shorten(text);
    if text.is_empty() { who } else { format!("{who}: {text}") }
}

/// `text` on one line, cut to [`PREVIEW_CHARS`] characters.
pub fn shorten(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let line = words.join(" ");
    match line.char_indices().nth(PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}…", line[..cut].trim_end()),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use twitty_model::{
        Backend, FeedSource, HomeMode, NotificationFilter, PageRequest, mock::MockBackend,
    };

    fn page(source: FeedSource) -> Vec<FeedItem> {
        let backend = MockBackend::new();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(backend.feed(&source, PageRequest::First))
            .unwrap()
            .items
    }

    fn home() -> Vec<FeedItem> {
        page(FeedSource::Home { mode: HomeMode::Following })
    }

    fn prepended(count: usize) -> ListChange {
        ListChange::Splice { range: 0..0, count }
    }

    #[test]
    fn newer_rows_are_announced() {
        let items = home();
        let one = announce(LoadKind::Newer, &prepended(1), &items).unwrap();
        assert_eq!(one.count, 1);
        assert_eq!(one.body, preview(&items[0]));
        assert!(!one.body.is_empty());

        let three = announce(LoadKind::Newer, &prepended(3), &items).unwrap();
        assert_eq!(three.count, 3);
        assert_eq!(three.body, format!("3 new posts\n{}", preview(&items[0])));
    }

    #[test]
    fn other_loads_are_not_news() {
        let items = home();
        let n = items.len();
        assert_eq!(announce(LoadKind::Refresh, &ListChange::Reset { count: n }, &items), None);
        assert_eq!(
            announce(LoadKind::Older, &ListChange::Splice { range: 5..5, count: 3 }, &items),
            None
        );
        assert_eq!(announce(LoadKind::Newer, &ListChange::None, &items), None);
        assert_eq!(announce(LoadKind::Newer, &prepended(0), &items), None);
        assert_eq!(announce(LoadKind::Newer, &prepended(n + 1), &items), None, "out of range");
    }

    #[test]
    fn only_readable_rows_count() {
        let mut items = vec![
            FeedItem::Unsupported { entry: "a".into(), reason: "new shape".into() },
            FeedItem::Unavailable {
                entry: "b".into(),
                id: None,
                reason: twitty_model::UnavailableReason::Deleted,
            },
        ];
        assert_eq!(announce(LoadKind::Newer, &prepended(2), &items), None);
        items.insert(1, home().remove(0));
        let one = announce(LoadKind::Newer, &prepended(3), &items).unwrap();
        assert_eq!(one.count, 1);
    }

    #[test]
    fn notifications_say_notifications() {
        let items = page(FeedSource::Notifications { filter: NotificationFilter::All });
        let notes = items.iter().take_while(|i| matches!(i, FeedItem::Notification(_))).count();
        assert!(notes >= 2, "the mock starts with notifications");
        let a = announce(LoadKind::Newer, &prepended(2), &items).unwrap();
        assert!(a.body.starts_with("2 new notifications\n"), "{}", a.body);
    }

    #[test]
    fn previews_are_one_short_line() {
        assert_eq!(shorten("  hello\n\nworld  "), "hello world");
        let long = "word ".repeat(100);
        let cut = shorten(&long);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= PREVIEW_CHARS + 1);
    }

    proptest! {
        #[test]
        fn shorten_never_panics_and_stays_short(text in "\\PC{0,400}") {
            let line = shorten(&text);
            prop_assert!(line.chars().count() <= PREVIEW_CHARS + 1);
            prop_assert!(!line.contains('\n'));
        }
    }
}
