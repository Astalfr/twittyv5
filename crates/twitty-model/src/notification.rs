use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{CommunityNote, NotificationId, Post, Profile, RichText};

/// An activity notification (likes, follows, retweets…).
///
/// Replies and mentions are delivered as [`crate::FeedItem::Post`] instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: NotificationId,
    pub kind: NotificationKind,
    pub created_at: DateTime<Utc>,
    /// Who did it, most recent first. May be empty for system notices.
    pub actors: Vec<Arc<Profile>>,
    /// Summary line, e.g. "Alice and 3 others liked your tweet".
    pub text: RichText,
    /// The tweet the notification is about, when there is one.
    pub target: Option<Box<Post>>,
    /// The Community Note a [`NotificationKind::CommunityNote`] is about.
    pub note: Option<CommunityNote>,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Like,
    Repost,
    Follow,
    Quote,
    /// Readers added a Community Note to a tweet the viewer interacted with.
    CommunityNote,
    /// Anything Twitty has no dedicated presentation for.
    Other,
}
