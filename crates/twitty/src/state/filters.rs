//! Column filters: which kinds of tweets a column shows.
//!
//! A row is hidden when any of its kinds is hidden. Its kinds are those of all
//! its tweets (a thread is a reply, a retweet of a picture has images), but
//! never those of a quoted tweet. Rows that are not tweets (activity
//! notifications, unavailable or unsupported rows) are always shown.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use twitty_model::{Embed, FeedItem, Media, Post};

/// A kind of tweet a column can hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TweetKind {
    /// No pictures, videos or quoted tweet (a link card or a poll is fine).
    TextOnly,
    Images,
    /// Videos and GIFs.
    Videos,
    Replies,
    Quotes,
    Retweets,
}

impl TweetKind {
    /// Every kind, in the order the settings dialog shows them.
    pub const ALL: [TweetKind; 6] =
        [Self::TextOnly, Self::Images, Self::Videos, Self::Replies, Self::Quotes, Self::Retweets];

    /// The name it is stored under.
    pub fn key(self) -> &'static str {
        match self {
            Self::TextOnly => "text_only",
            Self::Images => "images",
            Self::Videos => "videos",
            Self::Replies => "replies",
            Self::Quotes => "quotes",
            Self::Retweets => "retweets",
        }
    }

    /// The label of its switch in the settings dialog.
    pub fn label(self) -> &'static str {
        match self {
            Self::TextOnly => "Text-only tweets",
            Self::Images => "Tweets with images",
            Self::Videos => "Tweets with videos",
            Self::Replies => "Replies",
            Self::Quotes => "Quotes",
            Self::Retweets => "Retweets",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }
}

/// The kinds a column hides.
///
/// Stored as a list of names; a name this version doesn't know is dropped, so
/// a file from a newer app never fails to load over a filter.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "Vec<String>", into = "Vec<String>")]
pub struct HiddenKinds(BTreeSet<TweetKind>);

impl HiddenKinds {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for HiddenKinds {
    fn from(keys: Vec<String>) -> Self {
        Self(keys.iter().filter_map(|key| TweetKind::from_key(key)).collect())
    }
}

impl From<HiddenKinds> for Vec<String> {
    fn from(hidden: HiddenKinds) -> Self {
        hidden.0.into_iter().map(|kind| kind.key().to_owned()).collect()
    }
}

/// Which tweets a column shows. By default, all of them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnFilters {
    /// The "Show advanced filters" switch. Off, every tweet shows, and the
    /// hidden kinds are kept for when it is turned on again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    #[serde(skip_serializing_if = "HiddenKinds::is_empty")]
    hidden: HiddenKinds,
}

impl ColumnFilters {
    /// Advanced filters on, hiding `kinds`.
    pub fn hiding(kinds: &[TweetKind]) -> Self {
        Self { advanced: true, hidden: HiddenKinds(kinds.iter().copied().collect()) }
    }

    /// Whether nothing differs from the defaults (nothing to save).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the `kind` switch is on (ignoring [`Self::advanced`]).
    pub fn shows_kind(&self, kind: TweetKind) -> bool {
        !self.hidden.0.contains(&kind)
    }

    /// Turns the `kind` switch on or off.
    pub fn set_shown(&mut self, kind: TweetKind, shown: bool) {
        if shown {
            self.hidden.0.remove(&kind);
        } else {
            self.hidden.0.insert(kind);
        }
    }

    /// Whether any row can be hidden.
    pub fn is_active(&self) -> bool {
        self.advanced && !self.hidden.is_empty()
    }

    /// Whether the column shows `item`.
    pub fn shows(&self, item: &FeedItem) -> bool {
        !self.is_active() || kinds(item).iter().all(|kind| self.shows_kind(*kind))
    }
}

/// The kinds of tweet `item` is; empty for rows that are not tweets.
pub fn kinds(item: &FeedItem) -> BTreeSet<TweetKind> {
    let mut kinds = BTreeSet::new();
    let posts: Vec<&Post> = match item {
        FeedItem::Repost { post, .. } => {
            kinds.insert(TweetKind::Retweets);
            vec![post]
        }
        FeedItem::Post(post) | FeedItem::Pinned(post) => vec![post],
        FeedItem::Thread(posts) => posts.iter().collect(),
        FeedItem::Notification(_) | FeedItem::Unavailable { .. } | FeedItem::Unsupported { .. } => {
            return kinds;
        }
    };
    for post in posts {
        if post.reply_to.is_some() {
            kinds.insert(TweetKind::Replies);
        }
        if matches!(post.embed, Some(Embed::Quote { .. })) {
            kinds.insert(TweetKind::Quotes);
        }
        for media in &post.media {
            kinds.insert(match media {
                Media::Image { .. } => TweetKind::Images,
                Media::Video { .. } | Media::Gif { .. } => TweetKind::Videos,
            });
        }
    }
    let rich = [TweetKind::Images, TweetKind::Videos, TweetKind::Quotes];
    if !rich.iter().any(|kind| kinds.contains(kind)) {
        kinds.insert(TweetKind::TextOnly);
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use twitty_model::{
        Backend, FeedSource, HomeMode, ImageRef, Notification, NotificationId, NotificationKind,
        PageRequest, PostOrGone, Profile, ReplyRef, UnavailableReason, UserId, mock::MockBackend,
    };

    /// A plain text tweet: no media, no embed, not a reply.
    fn text() -> Post {
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                MockBackend::new()
                    .feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First),
            )
            .unwrap();
        let mut post = page.items[0].posts()[0].clone();
        post.media.clear();
        post.embed = None;
        post.reply_to = None;
        post
    }

    fn image(post: &Post) -> ImageRef {
        ImageRef { url: post.url.clone(), size: None, crops: vec![] }
    }

    fn with_image(mut post: Post) -> Post {
        post.media.push(Media::Image { image: image(&post), alt: None });
        post
    }

    fn with_gif(mut post: Post) -> Post {
        post.media.push(Media::Gif { poster: image(&post), stream: None, alt: None });
        post
    }

    fn reply(mut post: Post) -> Post {
        post.reply_to = Some(ReplyRef { post: None, user: None, handle: None });
        post
    }

    fn quoting(mut post: Post, quoted: Post) -> Post {
        post.embed = Some(Embed::Quote { quoted: Box::new(PostOrGone::Post(quoted)) });
        post
    }

    fn kinds_of(item: &FeedItem) -> Vec<TweetKind> {
        kinds(item).into_iter().collect()
    }

    use TweetKind::{Images, Quotes, Replies, Retweets, TextOnly, Videos};

    #[test]
    fn tweets_are_classified() {
        assert_eq!(kinds_of(&FeedItem::Post(text())), [TextOnly]);
        assert_eq!(kinds_of(&FeedItem::Pinned(with_image(text()))), [Images]);
        assert_eq!(kinds_of(&FeedItem::Post(with_gif(text()))), [Videos], "GIFs are videos");
        assert_eq!(
            kinds_of(&FeedItem::Post(with_gif(with_image(text())))),
            [Images, Videos],
            "mixed media"
        );
        assert_eq!(kinds_of(&FeedItem::Post(reply(text()))), [TextOnly, Replies]);
        let by = Arc::new(Profile::new(UserId::new("9"), "rt", "Retweeter"));
        assert_eq!(
            kinds_of(&FeedItem::Repost { by, post: with_image(text()) }),
            [Images, Retweets],
            "a retweet has the tweet's kinds"
        );
    }

    #[test]
    fn a_quote_has_its_own_kinds_not_the_quoted_tweets() {
        let quote = quoting(text(), reply(with_image(text())));
        assert_eq!(kinds_of(&FeedItem::Post(quote)), [Quotes], "not text-only, no images");
    }

    #[test]
    fn a_thread_has_the_kinds_of_all_its_tweets() {
        let thread = FeedItem::Thread(vec![text(), reply(with_image(text()))]);
        assert_eq!(kinds_of(&thread), [Images, Replies], "text-only only when no tweet has media");
        let plain = FeedItem::Thread(vec![text(), reply(text())]);
        assert_eq!(kinds_of(&plain), [TextOnly, Replies]);
    }

    #[test]
    fn rows_that_are_not_tweets_have_no_kind_and_always_show() {
        let gone = FeedItem::Unavailable {
            entry: "e".into(),
            id: None,
            reason: UnavailableReason::Deleted,
        };
        let notification = FeedItem::Notification(Notification {
            id: NotificationId::new("n"),
            kind: NotificationKind::Like,
            created_at: chrono::Utc::now(),
            actors: vec![],
            text: twitty_model::RichText::plain("liked your tweet"),
            target: Some(Box::new(text())),
            note: None,
        });
        let filters = ColumnFilters::hiding(&TweetKind::ALL);
        for item in [gone, notification] {
            assert!(kinds(&item).is_empty());
            assert!(filters.shows(&item));
        }
    }

    #[test]
    fn a_row_is_hidden_when_any_of_its_kinds_is() {
        let mut filters = ColumnFilters::default();
        let item = FeedItem::Post(reply(with_image(text())));
        assert!(filters.shows(&item), "everything shows by default");
        filters.set_shown(Replies, false);
        assert!(filters.shows(&item), "the filters only apply with the advanced switch on");
        filters.advanced = true;
        assert!(!filters.shows(&item));
        assert!(filters.shows(&FeedItem::Post(with_image(text()))));
        filters.set_shown(Replies, true);
        filters.set_shown(TextOnly, false);
        assert!(filters.shows(&item));
        assert!(!filters.shows(&FeedItem::Post(text())));
    }

    #[test]
    fn filters_round_trip_and_skip_defaults() {
        assert_eq!(serde_json::to_value(ColumnFilters::default()).unwrap(), serde_json::json!({}));
        let filters = ColumnFilters::hiding(&[Retweets, Images]);
        let json = serde_json::to_value(&filters).unwrap();
        assert_eq!(json, serde_json::json!({"advanced": true, "hidden": ["images", "retweets"]}));
        assert_eq!(serde_json::from_value::<ColumnFilters>(json).unwrap(), filters);
    }

    #[test]
    fn unknown_kinds_and_missing_fields_load() {
        let filters: ColumnFilters =
            serde_json::from_value(serde_json::json!({"hidden": ["polls", "quotes"]})).unwrap();
        assert!(!filters.advanced, "missing fields take their defaults");
        let shown: Vec<_> =
            TweetKind::ALL.into_iter().filter(|k| !filters.shows_kind(*k)).collect();
        assert_eq!(shown, [Quotes], "unknown names are dropped");
    }

    #[test]
    fn kind_keys_are_unique() {
        let mut keys: Vec<_> = TweetKind::ALL.iter().map(|k| k.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TweetKind::ALL.len());
        for kind in TweetKind::ALL {
            assert_eq!(TweetKind::from_key(kind.key()), Some(kind));
        }
    }

    #[test]
    fn a_live_following_timeline_loses_its_retweets() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../twitty-x/tests/fixtures/home--latest-retweets.json"
        );
        let raw = std::fs::read_to_string(path).unwrap();
        let items = twitty_x::translate::feed_page(&serde_json::from_str(&raw).unwrap()).page.items;
        let is_retweet = |i: &&FeedItem| matches!(i, FeedItem::Repost { .. });
        let retweets = items.iter().filter(is_retweet).count();
        assert!(retweets > 0, "the fixture has retweets");

        let filters = ColumnFilters::hiding(&[Retweets]);
        let shown: Vec<_> = items.iter().filter(|i| filters.shows(i)).collect();
        assert!(!shown.iter().any(is_retweet));
        assert_eq!(shown.len(), items.len() - retweets, "only the retweets go");
    }
}
