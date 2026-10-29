//! Twitter JSON → `twitty-model`. The only code that knows both worlds.
//!
//! Rules (see `docs/domain-model.md`): never panic, never fail a whole page.
//! An entry that can't be translated becomes `FeedItem::Unsupported` with a
//! warning; entries Twitty deliberately doesn't show (ads, prompts, who-to-follow)
//! are skipped.

mod card;
mod list;
mod media;
mod notification;
mod text;
mod tweet;
mod user;

use std::{collections::HashMap, sync::Arc};

use serde_json::Value;
use twitty_model::{
    FeedItem, FeedPage, FollowAction, FollowStatus, NotificationId, PageToken, Post, PostId,
    Profile, ThreadView,
};

pub use list::{TranslatedLists, lists};

use crate::wire::{self, CursorKind, Entry, at, bool_at, str_at};

/// Per-response translation state.
#[derive(Debug, Default)]
pub(crate) struct Ctx {
    /// Profiles by user id, so one response shares one `Arc` per author.
    users: HashMap<String, Arc<Profile>>,
    /// Problems met while translating, for diagnostics and tests.
    pub warnings: Vec<String>,
    /// Notifications whose tweet must be looked up through their note.
    pub(crate) notes: Vec<NoteLookup>,
}

/// A Community Notes notification that names its note but not the tweet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NoteLookup {
    pub(crate) notification: NotificationId,
    /// Twitter's note id, for `BirdwatchFetchOneNote`.
    pub(crate) note: String,
}

impl Ctx {
    pub(crate) fn profile(&mut self, user: &Value) -> Option<Arc<Profile>> {
        let id = str_at(user, &["rest_id"])?;
        if let Some(p) = self.users.get(id) {
            return Some(Arc::clone(p));
        }
        let p = Arc::new(user::profile(user)?);
        self.users.insert(id.to_owned(), Arc::clone(&p));
        Some(p)
    }

    pub(crate) fn warn(&mut self, message: String) {
        tracing::warn!("translate: {message}");
        self.warnings.push(message);
    }

    pub(crate) fn unsupported(&mut self, entry: &str, reason: impl Into<String>) -> FeedItem {
        let reason = reason.into();
        self.warn(format!("{entry}: {reason}"));
        FeedItem::Unsupported { entry: entry.to_owned(), reason }
    }
}

/// Item types Twitty intentionally doesn't show.
const SKIPPED_ITEMS: &[&str] = &[
    "TimelineUser",
    "TimelineMessagePrompt",
    "TimelinePrompt",
    "TimelineRelevancePrompt",
    "TimelineTrend",
    "TimelineTopic",
    "TimelineLabel",
    "TimelineSpelling",
    "TimelineCommunity",
    "TimelineTwitterList",
    "TimelineEventSummary",
    "TimelineTimelineCursor",
    "TimelineHomeConversationFeedback",
    "TimelineTombstone",
];

/// Result of translating one response.
#[derive(Debug)]
pub struct Translated {
    pub page: FeedPage,
    /// Entries that were translated as `Unsupported` or partially, with reasons.
    pub warnings: Vec<String>,
    /// Notifications of the page whose tweet the backend still has to find.
    pub(crate) notes: Vec<NoteLookup>,
}

/// Translates any timeline response (home, search, user, bookmarks,
/// notifications, tweet detail) into a page of rows.
pub fn feed_page(root: &Value) -> Translated {
    let mut ctx = Ctx::default();
    let mut page = FeedPage::default();
    for entry in wire::timeline(root) {
        match entry {
            Entry::Cursor { kind: CursorKind::Top, value } => {
                page.newer = Some(PageToken::new(value))
            }
            Entry::Cursor { kind: CursorKind::Bottom, value } => {
                page.older = Some(PageToken::new(value));
            }
            Entry::Item { id, content } => page.items.extend(item(id, content, &mut ctx)),
            // A pinned tombstone stays a tombstone.
            Entry::Pinned { id, content } => {
                page.items.extend(item(id, content, &mut ctx).map(|row| match row {
                    FeedItem::Post(post) => FeedItem::Pinned(post),
                    other => other,
                }))
            }
            Entry::Module { id, display_type, items } => {
                page.items.extend(module(id, display_type, items, &mut ctx));
            }
            Entry::Other { id } => tracing::debug!("skipping entry {id}"),
        }
    }
    Translated { page, warnings: ctx.warnings, notes: ctx.notes }
}

/// The tweet a `BirdwatchFetchOneNote` response's note is attached to.
/// `None` for a media note, which Twitter attaches to an image (shared by
/// many tweets) rather than to one tweet.
pub(crate) fn note_post(root: &Value) -> Option<PostId> {
    let note = at(root, &["data", "birdwatch_note_by_rest_id"])?;
    str_at(note, &["tweet_results", "result", "rest_id"]).map(PostId::new)
}

/// The signed-in account from a `Viewer` response.
pub fn viewer(root: &Value) -> Option<Profile> {
    user::profile(at(root, &["data", "viewer", "user_results", "result"])?)
}

/// Result of translating a list of accounts.
#[derive(Debug)]
pub struct TranslatedUsers {
    pub users: Vec<Profile>,
    /// Entries that could not be read as an account, with reasons.
    pub warnings: Vec<String>,
}

/// The accounts of a `SearchTimeline` "People" response, in Twitter's order. An
/// entry that isn't a readable account is skipped with a warning.
pub fn users(root: &Value) -> TranslatedUsers {
    let mut ctx = Ctx::default();
    let mut users = Vec::new();
    for entry in wire::timeline(root) {
        let Entry::Item { id, content } = entry else { continue };
        match str_at(content, &["itemType"]) {
            Some("TimelineUser") => {}
            // Prompts, spelling suggestions…: known, just not accounts.
            Some(t) if SKIPPED_ITEMS.contains(&t) => continue,
            t => {
                ctx.warn(format!("{id}: not an account ({t:?})"));
                continue;
            }
        }
        match at(content, &["user_results", "result"]).and_then(user::profile) {
            Some(profile) => users.push(profile),
            None => ctx.warn(format!("{id}: unreadable account")),
        }
    }
    TranslatedUsers { users, warnings: ctx.warnings }
}

/// The profile from a `UserByScreenName` response.
pub fn user(root: &Value) -> Option<Profile> {
    user::profile(at(root, &["data", "user", "result"])?)
}

/// Splits a `TweetDetail` response around the `focal` tweet: rows before it are
/// its ancestors, rows after it the replies. `None` when the focal tweet isn't
/// in the response (deleted, protected, withheld…).
pub fn thread(root: &Value, focal: &PostId) -> Option<ThreadView> {
    let page = feed_page(root).page;
    let at_focus = |item: &FeedItem| matches!(item, FeedItem::Post(p) if &p.id == focal);
    let split = page.items.iter().position(at_focus)?;
    let mut items = page.items.into_iter();
    let ancestors = items
        .by_ref()
        .take(split)
        .flat_map(|item| item.posts().into_iter().cloned().collect::<Vec<_>>())
        .collect();
    let Some(FeedItem::Post(focus)) = items.next() else { return None };
    Some(ThreadView { ancestors, focus, replies: items.collect(), more_replies: page.older })
}

/// Where the viewer stands after a v1.1 `friendships/create` or `destroy`
/// call, from the user object it returns. Twitter's `following` there can still
/// show the state before the call, so a follow is trusted unless Twitter says it
/// is only a request (a protected account).
pub fn follow_status(root: &Value, action: FollowAction) -> FollowStatus {
    match action {
        FollowAction::Unfollow => FollowStatus::NotFollowing,
        FollowAction::Follow => {
            let requested = bool_at(root, &["follow_request_sent"]) == Some(true);
            let protected = bool_at(root, &["protected"]) == Some(true);
            let following = bool_at(root, &["following"]) == Some(true);
            if requested || (protected && !following) {
                FollowStatus::Requested
            } else {
                FollowStatus::Following
            }
        }
    }
}

/// Result of translating a lookup of tweets by id.
#[derive(Debug)]
pub struct TranslatedPosts {
    pub posts: Vec<Post>,
    /// Results that could not be read as a tweet, with reasons.
    pub warnings: Vec<String>,
}

/// The tweets of a `TweetResultsByRestIds` response. Twitter answers each id
/// in order, with an empty object for a tweet that doesn't exist; those and
/// tombstones are left out, unreadable results too (with a warning).
pub fn posts(root: &Value) -> TranslatedPosts {
    let mut ctx = Ctx::default();
    let mut posts = Vec::new();
    let Some(results) = at(root, &["data", "tweetResult"]).and_then(Value::as_array) else {
        ctx.warn("response has no tweetResult array".into());
        return TranslatedPosts { posts, warnings: ctx.warnings };
    };
    for (ix, slot) in results.iter().enumerate() {
        let Some(result) = at(slot, &["result"]) else { continue };
        match tweet::unwrap(result) {
            tweet::Unwrapped::Tweet(t) => match tweet::post(t, &mut ctx, 0) {
                Ok(post) => posts.push(post),
                Err(e) => ctx.warn(format!("tweetResult[{ix}]: {e}")),
            },
            tweet::Unwrapped::Gone(_) => {}
            tweet::Unwrapped::Unknown(e) => ctx.warn(format!("tweetResult[{ix}]: {e}")),
        }
    }
    TranslatedPosts { posts, warnings: ctx.warnings }
}

/// The tweet a `CreateTweet` response returns.
pub fn created_post(root: &Value) -> Option<Post> {
    let result = at(root, &["data", "create_tweet", "tweet_results", "result"])?;
    match tweet::item(result, "created", &mut Ctx::default()) {
        FeedItem::Post(post) => Some(post),
        _ => None,
    }
}

fn is_promoted(entry: &str, content: &Value) -> bool {
    entry.starts_with("promoted-") || at(content, &["promotedMetadata"]).is_some()
}

fn item(entry: &str, content: &Value, ctx: &mut Ctx) -> Option<FeedItem> {
    if is_promoted(entry, content) {
        return None;
    }
    match str_at(content, &["__typename"]).or_else(|| str_at(content, &["itemType"])) {
        // Empty results happen for tweets the viewer can't see at all.
        Some("TimelineTweet") => {
            at(content, &["tweet_results", "result"]).map(|r| tweet::item(r, entry, ctx))
        }
        Some("TimelineNotification") => Some(notification::item(content, entry, ctx)),
        Some(t) if SKIPPED_ITEMS.contains(&t) => None,
        Some(t) => Some(ctx.unsupported(entry, format!("unknown item type {t}"))),
        None => Some(ctx.unsupported(entry, "item without type")),
    }
}

/// Conversation modules become threads; other modules (who-to-follow,
/// carousels) are shown only for the tweets they contain.
fn module(entry: &str, display_type: &str, items: &[Value], ctx: &mut Ctx) -> Vec<FeedItem> {
    let rows: Vec<FeedItem> =
        wire::module_items(items).filter_map(|(id, content)| item(id, content, ctx)).collect();
    if !display_type.contains("Conversation") {
        return rows;
    }
    // A conversation is shown as one connected group of tweets.
    let (posts, others): (Vec<FeedItem>, Vec<FeedItem>) =
        rows.into_iter().partition(|r| matches!(r, FeedItem::Post(_)));
    let mut posts: Vec<Post> = posts
        .into_iter()
        .filter_map(|r| match r {
            FeedItem::Post(p) => Some(p),
            _ => None,
        })
        .collect();
    let mut out = others;
    match posts.len() {
        0 => {}
        1 => out.insert(0, FeedItem::Post(posts.remove(0))),
        _ => {
            tracing::trace!("thread module {entry}");
            out.insert(0, FeedItem::Thread(posts));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_note_lookup_names_its_tweet_and_a_media_note_none() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/birdwatch-note.json")).unwrap();
        assert_eq!(note_post(&fixture), Some(PostId::new("7900511420478210047")));
        // A media note, attached to an image rather than to one tweet.
        let media = json!({"data": {"birdwatch_note_by_rest_id": {
            "rest_id": "1", "is_media_note": true, "tweet_results": {}
        }}});
        assert_eq!(note_post(&media), None);
        assert_eq!(note_post(&json!({"data": {}})), None);
    }

    #[test]
    fn a_pinned_tweet_is_marked_and_a_pinned_tombstone_stays_one() {
        let pin = |result| {
            json!({"type": "TimelinePinEntry", "entry": {"entryId": "tweet-1", "content": {
                "entryType": "TimelineTimelineItem",
                "itemContent": {"__typename": "TimelineTweet", "tweet_results": {"result": result}}
            }}})
        };
        let page = |result| {
            feed_page(&json!({"data": {"instructions": [pin(result)]}})).page.items.remove(0)
        };
        let gone = page(json!({"__typename": "TweetUnavailable", "reason": "Protected"}));
        assert!(matches!(gone, FeedItem::Unavailable { .. }), "{gone:?}");

        let fixture = include_str!("../../tests/fixtures/usertweets-nasa.json");
        let items = feed_page(&serde_json::from_str(fixture).unwrap()).page.items;
        assert!(matches!(items[0], FeedItem::Pinned(_)), "{:?}", items[0]);
        assert!(!items[1..].iter().any(|i| matches!(i, FeedItem::Pinned(_))));
    }

    #[test]
    fn follow_status_reads_requests_and_ignores_a_stale_following() {
        let follow = |body| follow_status(&body, FollowAction::Follow);
        // Twitter can echo the state before the call.
        assert_eq!(
            follow(json!({"protected": false, "following": false})),
            FollowStatus::Following
        );
        assert_eq!(follow(json!({"protected": true})), FollowStatus::Requested);
        assert_eq!(follow(json!({"follow_request_sent": true})), FollowStatus::Requested);
        assert_eq!(
            follow(json!({"protected": true, "following": true})),
            FollowStatus::Following,
            "already following a locked account"
        );
        assert_eq!(follow(json!({})), FollowStatus::Following, "nothing to go on");
        assert_eq!(follow_status(&json!({}), FollowAction::Unfollow), FollowStatus::NotFollowing);
    }
}
