//! [`XBackend`]: the `twitty-model` [`Backend`] port implemented over X.

use std::sync::Arc;

use serde_json::{Value, json};
use twitty_model::{
    Backend, BackendError, BoxFuture, Draft, FeedPage, FeedSource, FollowAction, FollowStatus,
    PageRequest, Post, PostAction, PostId, Profile, RateBudget, RequestHealth, ThreadView, UserId,
};

use crate::{XClient, XError, health::Health, requests, translate};

/// X's rate-limit window.
const WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Assumed limit before X reported one (the lowest seen for timelines is 50).
const ASSUMED_LIMIT: u32 = 50;

/// X error codes meaning the action already happened ("already favorited",
/// "already retweeted"). The request is idempotent from the UI's view.
const ALREADY_DONE: [i64; 2] = [139, 327];

/// X's code for "you've already requested to follow" (a protected account).
const ALREADY_REQUESTED: i64 = 160;

/// Health label of account searches, kept apart from the search columns'
/// `SearchTimeline` counters (both share X's rate limit).
const PEOPLE_SEARCH: &str = "SearchTimeline (People)";

/// X as a [`Backend`]. Cheap to clone; clones share the client.
#[derive(Debug, Clone)]
pub struct XBackend {
    client: Arc<XClient>,
    health: Arc<Health>,
}

impl XBackend {
    pub fn new(client: Arc<XClient>) -> Self {
        Self { client, health: Arc::new(Health::default()) }
    }

    /// Records the outcome of a request without rows.
    fn track<T>(&self, name: &str, result: Result<T, BackendError>) -> Result<T, BackendError> {
        match &result {
            Ok(_) => self.health.ok(name),
            Err(e) => self.health.error(name, e),
        }
        result
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
        let body = match self.client.graphql(operation, variables).await {
            Ok(body) => body,
            Err(e) => {
                self.health.error(operation, &e);
                return Err(e.into());
            }
        };
        let translated = translate::feed_page(&body);
        if translated.page.items.is_empty() && wire_has_no_timeline(&body) {
            self.health.warning(operation, "response has no timeline instructions".into());
        }
        self.health.page(operation, &translated.page.items, &translated.warnings);
        Ok(translated.page)
    }
}

/// Whether `body` lacks any `instructions` array: a sign X moved the timeline.
fn wire_has_no_timeline(body: &Value) -> bool {
    crate::wire::find_instructions(body).is_none()
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
            let result = async {
                let variables = json!({"withCommunitiesMemberships": true});
                let body = self.client.graphql("Viewer", variables).await?;
                translate::viewer(&body)
                    .ok_or_else(|| BackendError::Decode("no signed-in user in the response".into()))
            }
            .await;
            self.track("Viewer", result)
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
            let result = async {
                let variables = requests::tweet_detail_variables(id.as_str());
                let body = self.client.graphql("TweetDetail", variables).await?;
                translate::thread(&body, id).ok_or(BackendError::NotFound)
            }
            .await;
            self.track("TweetDetail", result)
        })
    }

    fn profile<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, Result<Profile, BackendError>> {
        Box::pin(async move {
            let result = async {
                let variables = requests::user_by_screen_name_variables(handle);
                let body = self.client.graphql("UserByScreenName", variables).await?;
                translate::user(&body).ok_or(BackendError::NotFound)
            }
            .await;
            self.track("UserByScreenName", result)
        })
    }

    fn search_users<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Profile>, BackendError>> {
        Box::pin(async move {
            let variables = requests::search_users_variables(query);
            match self.client.graphql("SearchTimeline", variables).await {
                Ok(body) => {
                    let translated = translate::users(&body);
                    let unreadable = translated.warnings.len();
                    self.health.rows(
                        PEOPLE_SEARCH,
                        translated.users.len() + unreadable,
                        unreadable,
                        &translated.warnings,
                    );
                    Ok(translated.users)
                }
                Err(e) => {
                    self.health.error(PEOPLE_SEARCH, &e);
                    Err(e.into())
                }
            }
        })
    }

    fn act(&self, action: PostAction) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(async move {
            let (operation, variables) = requests::action(&action);
            let result = match self.client.graphql(operation, variables).await {
                Ok(body) => mutation_outcome(&body),
                Err(XError::Api { code, .. }) if ALREADY_DONE.contains(&code) => Ok(()),
                Err(e) => Err(e.into()),
            };
            self.track(operation, result)
        })
    }

    fn set_following<'a>(
        &'a self,
        user: &'a UserId,
        action: FollowAction,
    ) -> BoxFuture<'a, Result<FollowStatus, BackendError>> {
        Box::pin(async move {
            let (path, fields) = requests::follow(user, action);
            let result = match self.client.rest_post(path, fields).await {
                Ok(body) => Ok(translate::follow_status(&body, action)),
                Err(XError::Api { code: ALREADY_REQUESTED, .. }) => Ok(FollowStatus::Requested),
                Err(e) => Err(e.into()),
            };
            self.track(path, result)
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
            let result = async {
                let body =
                    self.client.graphql("CreateTweet", requests::create_tweet(&draft)).await?;
                mutation_outcome(&body)?;
                translate::created_post(&body).ok_or_else(|| {
                    BackendError::Decode("no post in the CreateTweet response".into())
                })
            }
            .await;
            self.track("CreateTweet", result)
        })
    }

    fn health(&self) -> Vec<RequestHealth> {
        self.health.snapshot()
    }
}
