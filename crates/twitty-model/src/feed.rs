use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{ListId, Notification, PageToken, Post, PostId, Profile, UnavailableReason, UserId};

/// One row of a column.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedItem {
    Post(Post),
    /// `by` reposted `post`.
    Repost {
        by: Arc<Profile>,
        post: Post,
    },
    /// A conversation shown as a connected group, oldest first.
    Thread(Vec<Post>),
    Notification(Notification),
    /// A post that exists but cannot be shown.
    Unavailable {
        /// Backend-supplied row identity (e.g. a timeline entry id), unique within a feed.
        /// Needed because tombstones often carry no post id.
        entry: String,
        id: Option<PostId>,
        reason: UnavailableReason,
    },
    /// The backend received something it could not translate.
    Unsupported {
        /// Backend-supplied row identity, unique within a feed.
        entry: String,
        reason: String,
    },
}

impl FeedItem {
    /// A key that identifies the row across page loads, used for de-duplication.
    pub fn key(&self) -> String {
        match self {
            FeedItem::Post(p) => format!("post:{}", p.id),
            FeedItem::Repost { by, post } => format!("repost:{}:{}", by.id, post.id),
            FeedItem::Thread(posts) => match posts.last() {
                Some(p) => format!("thread:{}", p.id),
                None => "thread:empty".to_owned(),
            },
            FeedItem::Notification(n) => format!("notif:{}", n.id),
            FeedItem::Unavailable { entry, .. } => format!("gone:{entry}"),
            FeedItem::Unsupported { entry, .. } => format!("unsupported:{entry}"),
        }
    }

    /// Every post contained in this item, in display order.
    pub fn posts(&self) -> Vec<&Post> {
        match self {
            FeedItem::Post(p) | FeedItem::Repost { post: p, .. } => vec![p],
            FeedItem::Thread(posts) => posts.iter().collect(),
            FeedItem::Notification(n) => n.target.as_deref().into_iter().collect(),
            FeedItem::Unavailable { .. } | FeedItem::Unsupported { .. } => Vec::new(),
        }
    }
}

/// A page of rows plus cursors to continue in either direction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FeedPage {
    pub items: Vec<FeedItem>,
    /// Fetch items newer than this page.
    pub newer: Option<PageToken>,
    /// Fetch items older than this page.
    pub older: Option<PageToken>,
}

/// What a column shows. This type is persisted in deck files, so changes must
/// stay backward compatible (see `docs/domain-model.md`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedSource {
    Home {
        mode: HomeMode,
    },
    Notifications {
        #[serde(default)]
        filter: NotificationFilter,
    },
    Search {
        query: String,
        #[serde(default)]
        sort: SearchSort,
    },
    List {
        id: ListId,
        #[serde(default)]
        name: Option<String>,
    },
    User {
        id: UserId,
        handle: String,
        #[serde(default)]
        tab: UserTab,
    },
    Bookmarks,
    Thread {
        root: PostId,
    },
}

impl FeedSource {
    /// Default column title.
    pub fn title(&self) -> String {
        match self {
            FeedSource::Home { .. } => "Home".into(),
            FeedSource::Notifications { .. } => "Notifications".into(),
            FeedSource::Search { query, .. } => format!("Search: {query}"),
            FeedSource::List { name, .. } => name.clone().unwrap_or_else(|| "List".into()),
            FeedSource::User { handle, .. } => format!("@{handle}"),
            FeedSource::Bookmarks => "Bookmarks".into(),
            FeedSource::Thread { .. } => "Post".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeMode {
    /// Algorithmic timeline.
    #[default]
    ForYou,
    /// Chronological timeline of followed accounts.
    Following,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationFilter {
    #[default]
    All,
    Verified,
    Mentions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    Top,
    #[default]
    Latest,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserTab {
    #[default]
    Posts,
    Replies,
    Media,
    Likes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_source_json_is_stable() {
        // These strings are what deck files contain; changing them breaks users' saved decks.
        let cases = [
            (
                FeedSource::Home { mode: HomeMode::Following },
                r#"{"type":"home","mode":"following"}"#,
            ),
            (FeedSource::Bookmarks, r#"{"type":"bookmarks"}"#),
            (
                FeedSource::Search { query: "rust".into(), sort: SearchSort::Latest },
                r#"{"type":"search","query":"rust","sort":"latest"}"#,
            ),
        ];
        for (source, json) in cases {
            assert_eq!(serde_json::to_string(&source).unwrap(), json);
            assert_eq!(serde_json::from_str::<FeedSource>(json).unwrap(), source);
        }
    }

    #[test]
    fn tombstones_without_ids_keep_distinct_keys() {
        let gone = |entry: &str| FeedItem::Unavailable {
            entry: entry.into(),
            id: None,
            reason: UnavailableReason::Deleted,
        };
        assert_ne!(gone("e1").key(), gone("e2").key());
        let unsupported =
            |entry: &str| FeedItem::Unsupported { entry: entry.into(), reason: "same".into() };
        assert_ne!(unsupported("a").key(), unsupported("b").key());
    }

    #[test]
    fn feed_source_missing_optional_fields_default() {
        let s: FeedSource = serde_json::from_str(r#"{"type":"search","query":"q"}"#).unwrap();
        assert_eq!(s, FeedSource::Search { query: "q".into(), sort: SearchSort::Latest });
    }
}
