use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{NotificationId, Post, Profile, RichText};

/// An activity notification (likes, follows, reposts…).
///
/// Replies and mentions are delivered as [`crate::FeedItem::Post`] instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: NotificationId,
    pub kind: NotificationKind,
    pub created_at: DateTime<Utc>,
    /// Who did it, most recent first. May be empty for system notices.
    pub actors: Vec<Arc<Profile>>,
    /// Summary line, e.g. "Alice and 3 others liked your post".
    pub text: RichText,
    /// The post the notification is about, when there is one.
    pub target: Option<Box<Post>>,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Like,
    Repost,
    Follow,
    Quote,
    /// Anything Twitty has no dedicated presentation for.
    Other,
}
