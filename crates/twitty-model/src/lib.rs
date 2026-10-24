//! Twitty's stable domain model.
//!
//! Everything the app displays, stores or acts on is expressed with these
//! types. Network adapters (such as `twitty-x`) translate their own wire
//! formats into this model; nothing in here may mirror a provider's JSON.
//! See `docs/domain-model.md`.

#![forbid(unsafe_code)]

mod backend;
mod error;
mod feed;
mod ids;
mod media;
#[cfg(feature = "mock")]
pub mod mock;
mod notification;
mod post;
mod profile;
mod rich_text;

pub use backend::{Backend, BoxFuture, Draft, PageRequest, PostAction, ThreadView};
pub use error::BackendError;
pub use feed::{FeedItem, FeedPage, FeedSource, HomeMode, NotificationFilter, SearchSort, UserTab};
pub use ids::{ListId, NotificationId, PageToken, PostId, UserId};
pub use media::{AspectRatio, ImageRef, Media};
pub use notification::{Notification, NotificationKind};
pub use post::{
    Embed, LinkCard, Metrics, Poll, PollOption, Post, PostOrGone, ReplyRef, UnavailableReason,
    ViewerPostState,
};
pub use profile::{Badges, Profile, ProfileCounts, VerifiedKind, ViewerRelation};
pub use rich_text::{RichText, RichTextError, Segment, Span, SpanKind, UserRef};
