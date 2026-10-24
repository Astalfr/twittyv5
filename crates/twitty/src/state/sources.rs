//! Variants of a column's source shown as tabs under its header (Home's
//! "For you" / "Following", a profile's Posts / Replies / Media / Likes…).

use twitty_model::{FeedSource, HomeMode, NotificationFilter, SearchSort, UserTab};

/// One tab: its label and the source it switches the column to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub label: &'static str,
    pub source: FeedSource,
}

impl Tab {
    fn new(label: &'static str, source: FeedSource) -> Self {
        Self { label, source }
    }
}

/// The tabs for `source`, in display order; empty when it has no variants.
/// Exactly one tab equals `source` itself.
pub fn tabs(source: &FeedSource) -> Vec<Tab> {
    match source {
        FeedSource::Home { .. } => vec![
            Tab::new("For you", FeedSource::Home { mode: HomeMode::ForYou }),
            Tab::new("Following", FeedSource::Home { mode: HomeMode::Following }),
        ],
        FeedSource::Notifications { .. } => [
            ("All", NotificationFilter::All),
            ("Verified", NotificationFilter::Verified),
            ("Mentions", NotificationFilter::Mentions),
        ]
        .into_iter()
        .map(|(label, filter)| Tab::new(label, FeedSource::Notifications { filter }))
        .collect(),
        FeedSource::Search { query, .. } => {
            [("Top", SearchSort::Top), ("Latest", SearchSort::Latest), ("Media", SearchSort::Media)]
                .into_iter()
                .map(|(label, sort)| {
                    Tab::new(label, FeedSource::Search { query: query.clone(), sort })
                })
                .collect()
        }
        FeedSource::User { id, handle, .. } => [
            ("Posts", UserTab::Posts),
            ("Replies", UserTab::Replies),
            ("Media", UserTab::Media),
            ("Likes", UserTab::Likes),
        ]
        .into_iter()
        .map(|(label, tab)| {
            Tab::new(label, FeedSource::User { id: id.clone(), handle: handle.clone(), tab })
        })
        .collect(),
        FeedSource::List { .. } | FeedSource::Bookmarks | FeedSource::Thread { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{ListId, PostId, UserId};

    fn all_sources() -> Vec<FeedSource> {
        vec![
            FeedSource::Home { mode: HomeMode::Following },
            FeedSource::Notifications { filter: NotificationFilter::Mentions },
            FeedSource::Search { query: "rust".into(), sort: SearchSort::Media },
            FeedSource::User { id: UserId::new("1"), handle: "a".into(), tab: UserTab::Likes },
            FeedSource::List { id: ListId::new("2"), name: None },
            FeedSource::Bookmarks,
            FeedSource::Thread { root: PostId::new("3") },
        ]
    }

    #[test]
    fn exactly_one_tab_is_the_current_source() {
        for source in all_sources() {
            let tabs = tabs(&source);
            if tabs.is_empty() {
                continue;
            }
            assert_eq!(tabs.iter().filter(|t| t.source == source).count(), 1, "{source:?}");
        }
    }

    #[test]
    fn tabs_keep_the_query_and_user() {
        let search = FeedSource::Search { query: "gpui".into(), sort: SearchSort::Top };
        assert!(tabs(&search).iter().all(|t| t.source.title() == "Search: gpui"));
        let user =
            FeedSource::User { id: UserId::new("9"), handle: "NASA".into(), tab: UserTab::Posts };
        let labels: Vec<_> = tabs(&user).iter().map(|t| t.label).collect();
        assert_eq!(labels, ["Posts", "Replies", "Media", "Likes"]);
        assert!(tabs(&user).iter().all(|t| t.source.title() == "@NASA"));
    }

    #[test]
    fn single_view_sources_have_no_tabs() {
        assert!(tabs(&FeedSource::Bookmarks).is_empty());
        assert!(tabs(&FeedSource::List { id: ListId::new("2"), name: None }).is_empty());
    }
}
