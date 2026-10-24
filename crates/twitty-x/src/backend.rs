//! [`XBackend`]: the `twitty-model` [`Backend`] port implemented over X.

use std::sync::Arc;

use serde_json::{Value, json};
use twitty_model::{
    Backend, BackendError, BoxFuture, Draft, FeedPage, FeedSource, PageRequest, Post, PostAction,
    PostId, Profile, RateBudget, ThreadView,
};

use crate::{XClient, XError, requests, translate};

/// X's rate-limit window.
const WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Assumed limit before X reported one (the lowest seen for timelines is 50).
const ASSUMED_LIMIT: u32 = 50;

/// X error codes meaning the action already happened ("already favorited",
/// "already retweeted"). The request is idempotent from the UI's view.
const ALREADY_DONE: [i64; 2] = [139, 327];

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

/// A mutation succeeded unless X sent errors with empty `data` (the transport
/// keeps such bodies because queries use them for partial results).
fn mutation_outcome(body: &Value) -> Result<(), BackendError> {
    let errors = body.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty());
    let data_empty = body.get("data").and_then(Value::as_object).is_none_or(|d| d.is_empty());
    match errors.and_then(|e| e.first()) {
        Some(first) if data_empty => {
            let code = first.get("code").and_then(Value::as_i64).unwrap_or(0);
            if ALREADY_DONE.contains(&code) {
                return Ok(());
            }
            let message = first.get("message").and_then(Value::as_str).unwrap_or("unknown error");
            Err(BackendError::Upstream(format!("X error {code}: {message}")))
        }
        _ => Ok(()),
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

    fn act(&self, action: PostAction) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(async move {
            let (operation, variables) = requests::action(&action);
            match self.client.graphql(operation, variables).await {
                Ok(body) => mutation_outcome(&body),
                Err(XError::Api { code, .. }) if ALREADY_DONE.contains(&code) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
    }

    fn rate_budget(&self, source: &FeedSource) -> Option<RateBudget> {
        let (operation, _) = requests::feed(source, &PageRequest::First);
        let limit =
            self.client.transport().limits().get(operation).map_or(ASSUMED_LIMIT, |l| l.limit);
        Some(RateBudget { limit, window: WINDOW })
    }

    fn publish(&self, draft: Draft) -> BoxFuture<'_, Result<Post, BackendError>> {
        Box::pin(async move {
            let body = self.client.graphql("CreateTweet", requests::create_tweet(&draft)).await?;
            mutation_outcome(&body)?;
            translate::created_post(&body)
                .ok_or_else(|| BackendError::Decode("no post in the CreateTweet response".into()))
        })
    }
}
