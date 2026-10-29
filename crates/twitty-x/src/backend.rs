//! [`XBackend`]: the `twitty-model` [`Backend`] port implemented over Twitter.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use serde_json::{Value, json};
use twitty_model::{
    Backend, BackendError, BoxFuture, Draft, FeedItem, FeedPage, FeedSource, FollowAction,
    FollowStatus, ListSummary, PageRequest, Post, PostAction, PostId, Profile, RateBudget,
    RequestHealth, ThreadView, UserId,
};

use crate::{XClient, XError, health::Health, requests, translate, translate::NoteLookup};

/// Twitter's rate-limit window.
const WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Assumed limit before Twitter reported one (the lowest seen for timelines is 50).
const ASSUMED_LIMIT: u32 = 50;

/// Twitter error codes meaning the action already happened ("already favorited",
/// "already retweeted"). The request is idempotent from the UI's view.
const ALREADY_DONE: [i64; 2] = [139, 327];

/// Twitter's code for "you've already requested to follow" (a protected account).
const ALREADY_REQUESTED: i64 = 160;

/// Health label of account searches, kept apart from the search columns'
/// `SearchTimeline` counters (both share Twitter's rate limit).
const PEOPLE_SEARCH: &str = "SearchTimeline (People)";

/// Looks tweets up by id, for fresh counters.
const TWEET_RESULTS: &str = "TweetResultsByRestIds";

/// Ids per `TweetResultsByRestIds` request (90 were verified live).
const LOOKUP_BATCH: usize = 50;

/// Looks a Community Note up by id, for the tweet it is attached to.
const NOTE: &str = "BirdwatchFetchOneNote";

/// The viewer's lists page (owned, followed and suggested lists).
const LISTS_MANAGEMENT: &str = "ListsManagementPageTimeline";

/// Twitter as a [`Backend`]. Cheap to clone; clones share the client.
#[derive(Debug, Clone)]
pub struct XBackend {
    client: Arc<XClient>,
    health: Arc<Health>,
    /// The tweet each Community Note seen in notifications is attached to
    /// (`None` for a media note), so a refresh doesn't look it up again.
    noted: Arc<Mutex<HashMap<String, Option<PostId>>>>,
}

impl XBackend {
    pub fn new(client: Arc<XClient>) -> Self {
        Self {
            client,
            health: Arc::new(Health::default()),
            noted: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let mut page = translated.page;
        if !translated.notes.is_empty() {
            self.attach_noted_posts(&mut page.items, &translated.notes).await;
        }
        Ok(page)
    }

    /// Fills in the tweet of Community Notes notifications, which Twitter
    /// names only through their note. A failed lookup leaves the
    /// notification without its tweet, never fails the page.
    async fn attach_noted_posts(&self, items: &mut [FeedItem], lookups: &[NoteLookup]) {
        let mut wanted = Vec::new();
        for lookup in lookups {
            if let Some(post) = self.noted_post(&lookup.note).await {
                wanted.push((&lookup.notification, post));
            }
        }
        if wanted.is_empty() {
            return;
        }
        let ids: Vec<PostId> = wanted.iter().map(|(_, post)| post.clone()).collect();
        let posts = match self.posts(&ids).await {
            Ok(posts) => posts,
            Err(e) => {
                tracing::warn!("tweets of community notes: {e}");
                return;
            }
        };
        for item in items {
            let FeedItem::Notification(n) = item else { continue };
            let Some((_, id)) = wanted.iter().find(|(notification, _)| **notification == n.id)
            else {
                continue;
            };
            n.target = posts.iter().find(|p| p.id == *id).cloned().map(Box::new);
        }
    }

    /// The tweet `note` is attached to. Only answers are remembered, so a
    /// failed request is tried again with the next page.
    async fn noted_post(&self, note: &str) -> Option<PostId> {
        let known = self.noted.lock().unwrap_or_else(PoisonError::into_inner).get(note).cloned();
        if let Some(known) = known {
            return known;
        }
        match self.client.graphql(NOTE, json!({"note_id": note})).await {
            Ok(body) => {
                self.health.ok(NOTE);
                let post = translate::note_post(&body);
                self.noted
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(note.to_owned(), post.clone());
                post
            }
            Err(e) => {
                tracing::warn!("community note {note}: {e}");
                self.health.error(NOTE, &e);
                None
            }
        }
    }
}

/// Whether `body` lacks any `instructions` array: a sign Twitter moved the timeline.
fn wire_has_no_timeline(body: &Value) -> bool {
    crate::wire::find_instructions(body).is_none()
}

/// A mutation succeeded unless Twitter sent errors with empty `data` (the transport
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
            Err(BackendError::Upstream(format!("Twitter error {code}: {message}")))
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

    fn posts<'a>(&'a self, ids: &'a [PostId]) -> BoxFuture<'a, Result<Vec<Post>, BackendError>> {
        Box::pin(async move {
            let mut posts = Vec::with_capacity(ids.len());
            for batch in ids.chunks(LOOKUP_BATCH) {
                let variables = requests::tweet_results_variables(batch);
                let body = match self.client.graphql(TWEET_RESULTS, variables).await {
                    Ok(body) => body,
                    Err(e) => {
                        self.health.error(TWEET_RESULTS, &e);
                        return Err(e.into());
                    }
                };
                let translated = translate::posts(&body);
                let unreadable = translated.warnings.len();
                self.health.rows(
                    TWEET_RESULTS,
                    translated.posts.len() + unreadable,
                    unreadable,
                    &translated.warnings,
                );
                posts.extend(translated.posts);
            }
            Ok(posts)
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

    fn viewer_lists(&self) -> BoxFuture<'_, Result<Vec<ListSummary>, BackendError>> {
        Box::pin(async move {
            let variables = requests::lists_management_variables();
            match self.client.graphql(LISTS_MANAGEMENT, variables).await {
                Ok(body) => {
                    let translated = translate::lists(&body);
                    if !translated.found_module {
                        self.health.warning(
                            LISTS_MANAGEMENT,
                            "response has no 'Your Lists' module".into(),
                        );
                    }
                    let unreadable = translated.warnings.len();
                    self.health.rows(
                        LISTS_MANAGEMENT,
                        translated.lists.len() + unreadable,
                        unreadable,
                        &translated.warnings,
                    );
                    Ok(translated.lists)
                }
                Err(e) => {
                    self.health.error(LISTS_MANAGEMENT, &e);
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
