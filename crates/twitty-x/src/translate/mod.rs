//! X JSON → `twitty-model`. The only code that knows both worlds.
//!
//! Rules (see `docs/domain-model.md`): never panic, never fail a whole page.
//! An entry that can't be translated becomes `FeedItem::Unsupported` with a
//! warning; entries Twitty deliberately doesn't show (ads, prompts, who-to-follow)
//! are skipped.

mod card;
mod media;
mod notification;
mod text;
mod tweet;
mod user;

use std::{collections::HashMap, sync::Arc};

use serde_json::Value;
use twitty_model::{FeedItem, FeedPage, PageToken, Post, PostId, Profile, ThreadView};

use crate::wire::{self, CursorKind, Entry, at, str_at};

/// Per-response translation state.
#[derive(Debug, Default)]
pub(crate) struct Ctx {
    /// Profiles by user id, so one response shares one `Arc` per author.
    users: HashMap<String, Arc<Profile>>,
    /// Problems met while translating, for diagnostics and tests.
    pub warnings: Vec<String>,
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
            Entry::Module { id, display_type, items } => {
                page.items.extend(module(id, display_type, items, &mut ctx));
            }
            Entry::Other { id } => tracing::debug!("skipping entry {id}"),
        }
    }
    Translated { page, warnings: ctx.warnings }
}

/// The signed-in account from a `Viewer` response.
pub fn viewer(root: &Value) -> Option<Profile> {
    user::profile(at(root, &["data", "viewer", "user_results", "result"])?)
}

/// The profile from a `UserByScreenName` response.
pub fn user(root: &Value) -> Option<Profile> {
    user::profile(at(root, &["data", "user", "result"])?)
}

/// Splits a `TweetDetail` response around the `focal` post: rows before it are
/// its ancestors, rows after it the replies. `None` when the focal post isn't
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

fn is_promoted(entry: &str, content: &Value) -> bool {
    entry.starts_with("promoted-") || at(content, &["promotedMetadata"]).is_some()
}

fn item(entry: &str, content: &Value, ctx: &mut Ctx) -> Option<FeedItem> {
    if is_promoted(entry, content) {
        return None;
    }
    match str_at(content, &["__typename"]).or_else(|| str_at(content, &["itemType"])) {
        // Empty results happen for posts the viewer can't see at all.
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
/// carousels) are shown only for the posts they contain.
fn module(entry: &str, display_type: &str, items: &[Value], ctx: &mut Ctx) -> Vec<FeedItem> {
    let rows: Vec<FeedItem> =
        wire::module_items(items).filter_map(|(id, content)| item(id, content, ctx)).collect();
    if !display_type.contains("Conversation") {
        return rows;
    }
    // A conversation is shown as one connected group of posts.
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
