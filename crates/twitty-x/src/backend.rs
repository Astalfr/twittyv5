//! [`XBackend`]: the `twitty-model` [`Backend`] port implemented over X.

use std::sync::Arc;

use serde_json::json;
use twitty_model::{
    Backend, BackendError, BoxFuture, Draft, FeedPage, FeedSource, PageRequest, Post, PostAction,
    PostId, Profile, ThreadView,
};

use crate::{XClient, requests, translate};

/// X as a [`Backend`]. Cheap to clone; clones share the client.
#[derive(Debug, Clone)]
pub struct XBackend {
    client: Arc<XClient>,
}

impl XBackend {
    pub fn new(client: Arc<XClient>) -> Self {
        Self { client }
    }

    /// The underlying client (rate limits, session, catalog).
    pub fn client(&self) -> &XClient {
        &self.client
    }

    async fn feed_page(
        &self,
        source: &FeedSource,
        page: PageRequest,
    ) -> Result<FeedPage, BackendError> {
        let (operation, variables) = requests::feed(source, &page);
        let body = self.client.graphql(operation, variables).await?;
        Ok(translate::feed_page(&body).page)
    }
}

impl Backend for XBackend {
    fn viewer(&self) -> BoxFuture<'_, Result<Profile, BackendError>> {
        Box::pin(async move {
            let body =
                self.client.graphql("Viewer", json!({"withCommunitiesMemberships": true})).await?;
            translate::viewer(&body)
                .ok_or_else(|| BackendError::Decode("no signed-in user in the response".into()))
        })
    }

    fn feed<'a>(
        &'a self,
        source: &'a FeedSource,
        page: PageRequest,
    ) -> BoxFuture<'a, Result<FeedPage, BackendError>> {
        Box::pin(self.feed_page(source, page))
    }

    fn thread<'a>(&'a self, id: &'a PostId) -> BoxFuture<'a, Result<ThreadView, BackendError>> {
        Box::pin(async move {
            let variables = requests::tweet_detail_variables(id.as_str());
            let body = self.client.graphql("TweetDetail", variables).await?;
            translate::thread(&body, id).ok_or(BackendError::NotFound)
        })
    }

    fn profile<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, Result<Profile, BackendError>> {
        Box::pin(async move {
            let variables = requests::user_by_screen_name_variables(handle);
            let body = self.client.graphql("UserByScreenName", variables).await?;
            translate::user(&body).ok_or(BackendError::NotFound)
        })
    }

    // Writes to the account land in M8, with their own tests. Until then the
    // UI's optimistic toggles roll back on this error.
    fn act(&self, _action: PostAction) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(async {
            Err(BackendError::Upstream(
                "liking, reposting and bookmarking aren't enabled yet".into(),
            ))
        })
    }

    fn publish(&self, _draft: Draft) -> BoxFuture<'_, Result<Post, BackendError>> {
        Box::pin(async { Err(BackendError::Upstream("posting isn't enabled yet".into())) })
    }
}
