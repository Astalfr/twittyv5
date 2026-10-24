use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use url::Url;

use crate::{ImageRef, Media, PostId, Profile, RichText, UserId};

/// A single post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: PostId,
    pub author: Arc<Profile>,
    pub created_at: DateTime<Utc>,
    pub body: RichText,
    pub media: Vec<Media>,
    pub embed: Option<Embed>,
    pub reply_to: Option<ReplyRef>,
    pub metrics: Metrics,
    pub viewer: ViewerPostState,
    pub lang: Option<SmolStr>,
    /// The body machine-translated into the viewer's language, when the
    /// provider sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<Translation>,
    /// Canonical web link to the post.
    pub url: Url,
}

/// A machine translation of a post's body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translation {
    pub body: RichText,
    /// Language of the original body, as a BCP 47 code (`ja`, `pt-BR`…).
    pub from: Option<SmolStr>,
    /// Language of the translation, as a BCP 47 code.
    pub to: Option<SmolStr>,
}

/// What the post replies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyRef {
    pub post: Option<PostId>,
    pub user: Option<UserId>,
    /// Handle of the replied-to author, without `@`.
    pub handle: Option<SmolStr>,
}

/// Public engagement counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub replies: u64,
    pub reposts: u64,
    pub quotes: u64,
    pub likes: u64,
    pub bookmarks: u64,
    /// Not every post exposes a view count.
    pub views: Option<u64>,
}

/// The signed-in viewer's own state on a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewerPostState {
    pub liked: bool,
    pub reposted: bool,
    pub bookmarked: bool,
}

/// Content embedded below the post body.
#[expect(
    clippy::large_enum_variant,
    reason = "one embed per post; boxing adds indirection for no gain"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Embed {
    Quote { quoted: Box<PostOrGone> },
    Link(LinkCard),
    Poll(Poll),
}

/// A post that may have been removed or hidden.
#[expect(clippy::large_enum_variant, reason = "always stored behind a Box in Embed::Quote")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PostOrGone {
    Post(Post),
    Unavailable { reason: UnavailableReason },
}

/// Why a post cannot be shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum UnavailableReason {
    Deleted,
    Suspended,
    Protected,
    Withheld,
    /// Anything else, with a human readable explanation when available.
    Other(Option<String>),
}

/// A link preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkCard {
    pub url: Url,
    pub title: String,
    pub description: Option<String>,
    pub image: Option<ImageRef>,
    /// Domain shown under the card, e.g. `example.com`.
    pub domain: String,
}

/// A poll attached to a post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Poll {
    pub options: Vec<PollOption>,
    pub ends_at: Option<DateTime<Utc>>,
    pub closed: bool,
}

/// One choice of a [`Poll`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollOption {
    pub label: String,
    pub votes: u64,
}

impl Poll {
    /// Total number of votes across options.
    pub fn total_votes(&self) -> u64 {
        self.options.iter().map(|o| o.votes).sum()
    }
}
