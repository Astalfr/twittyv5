use std::{future::Future, pin::Pin};

use crate::{
    BackendError, FeedItem, FeedPage, FeedSource, ListSummary, PageToken, Post, PostId, Profile,
    UserId,
};

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

/// A conversation around one tweet.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadView {
    /// Tweets above the focused one, oldest first.
    pub ancestors: Vec<Post>,
    pub focus: Post,
    /// Replies below the focused tweet.
    pub replies: Vec<FeedItem>,
    pub more_replies: Option<PageToken>,
}

/// A change the viewer makes to a tweet.
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
    /// The tweet the action applies to.
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

/// A change the viewer makes to their relation with an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FollowAction {
    Follow,
    Unfollow,
}

/// Whether the viewer follows an account, after a [`FollowAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowStatus {
    NotFollowing,
    Following,
    /// The account is protected: it must approve the request first.
    Requested,
}

/// How many requests the provider allows for a feed per time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateBudget {
    pub limit: u32,
    pub window: std::time::Duration,
}

/// How one kind of request has gone since the app started, for the debug
/// panel. Rising `unsupported` or `warnings` mean the provider changed its
/// data and the adapter needs updating.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RequestHealth {
    /// The provider's name for the request (e.g. an operation name).
    pub name: String,
    /// Successful responses.
    pub ok: u64,
    /// Rows received.
    pub rows: u64,
    /// Rows the adapter could not understand.
    pub unsupported: u64,
    /// Problems met while translating (includes unsupported rows).
    pub warnings: u64,
    pub last_warning: Option<String>,
    /// Failed requests.
    pub errors: u64,
    pub last_error: Option<String>,
}

/// A tweet being composed.
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

    /// The conversation around a tweet.
    fn thread<'a>(&'a self, id: &'a PostId) -> BoxFuture<'a, Result<ThreadView, BackendError>>;

    /// The current state of tweets `ids` (counters, the viewer's likes…), in
    /// no particular order. Tweets that are gone or hidden are left out.
    fn posts<'a>(&'a self, ids: &'a [PostId]) -> BoxFuture<'a, Result<Vec<Post>, BackendError>>;

    /// A profile by handle (without `@`).
    fn profile<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, Result<Profile, BackendError>>;

    /// Accounts matching `query` (names, handles, bios), best match first.
    /// One page only; an empty list means nothing matched.
    fn search_users<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Profile>, BackendError>>;

    /// The lists the viewer owns or follows, in the provider's order. Lists
    /// it only suggests are left out.
    fn viewer_lists(&self) -> BoxFuture<'_, Result<Vec<ListSummary>, BackendError>>;

    /// Applies a viewer action to a tweet.
    fn act(&self, action: PostAction) -> BoxFuture<'_, Result<(), BackendError>>;

    /// Follows or unfollows `user`, returning where the viewer stands
    /// afterwards. Doing what is already done succeeds.
    fn set_following<'a>(
        &'a self,
        user: &'a UserId,
        action: FollowAction,
    ) -> BoxFuture<'a, Result<FollowStatus, BackendError>>;

    /// Publishes a new tweet and returns it.
    fn publish(&self, draft: Draft) -> BoxFuture<'_, Result<Post, BackendError>>;

    /// The request budget for loading `source`, if the provider has one.
    /// The app spaces out polling with it. `None` means unlimited.
    fn rate_budget(&self, _source: &FeedSource) -> Option<RateBudget> {
        None
    }

    /// Per-request health since startup, sorted by name. Empty when the
    /// backend doesn't track it.
    fn health(&self) -> Vec<RequestHealth> {
        Vec::new()
    }
}
