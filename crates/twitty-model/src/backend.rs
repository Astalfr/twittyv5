use std::{future::Future, pin::Pin};

use crate::{BackendError, FeedItem, FeedPage, FeedSource, PageToken, Post, PostId, Profile};

/// A boxed, sendable future, so [`Backend`] stays object safe.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which page of a feed to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRequest {
    /// The most recent page.
    First,
    /// Items newer than the token.
    Newer(PageToken),
    /// Items older than the token.
    Older(PageToken),
}

/// A conversation around one post.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadView {
    /// Posts above the focused one, oldest first.
    pub ancestors: Vec<Post>,
    pub focus: Post,
    /// Replies below the focused post.
    pub replies: Vec<FeedItem>,
    pub more_replies: Option<PageToken>,
}

/// A change the viewer makes to a post.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PostAction {
    Like(PostId),
    Unlike(PostId),
    Repost(PostId),
    Unrepost(PostId),
    Bookmark(PostId),
    Unbookmark(PostId),
}

impl PostAction {
    /// The post the action applies to.
    pub fn post_id(&self) -> &PostId {
        match self {
            PostAction::Like(id)
            | PostAction::Unlike(id)
            | PostAction::Repost(id)
            | PostAction::Unrepost(id)
            | PostAction::Bookmark(id)
            | PostAction::Unbookmark(id) => id,
        }
    }
}

/// How many requests the provider allows for a feed per time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateBudget {
    pub limit: u32,
    pub window: std::time::Duration,
}

/// A post being composed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Draft {
    pub text: String,
    pub reply_to: Option<PostId>,
    pub quote: Option<PostId>,
}

/// The port through which the app reaches a social network.
///
/// Implementations must be cheap to share (`Arc<dyn Backend>`) and their
/// futures must be `Send` so they can run on a background runtime.
pub trait Backend: Send + Sync + 'static {
    /// The signed-in account; doubles as a session check.
    fn viewer(&self) -> BoxFuture<'_, Result<Profile, BackendError>>;

    /// A page of a feed.
    fn feed<'a>(
        &'a self,
        source: &'a FeedSource,
        page: PageRequest,
    ) -> BoxFuture<'a, Result<FeedPage, BackendError>>;

    /// The conversation around a post.
    fn thread<'a>(&'a self, id: &'a PostId) -> BoxFuture<'a, Result<ThreadView, BackendError>>;

    /// A profile by handle (without `@`).
    fn profile<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, Result<Profile, BackendError>>;

    /// Applies a viewer action to a post.
    fn act(&self, action: PostAction) -> BoxFuture<'_, Result<(), BackendError>>;

    /// Publishes a new post and returns it.
    fn publish(&self, draft: Draft) -> BoxFuture<'_, Result<Post, BackendError>>;

    /// The request budget for loading `source`, if the provider has one.
    /// The app spaces out polling with it. `None` means unlimited.
    fn rate_budget(&self, _source: &FeedSource) -> Option<RateBudget> {
        None
    }
}
