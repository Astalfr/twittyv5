//! Tweet results → `Post` / `FeedItem`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use twitty_model::{
    Embed, FeedItem, Metrics, Post, PostId, PostOrGone, Profile, ReplyRef, UnavailableReason,
    UserId, ViewerPostState,
};
use url::Url;

use super::{Ctx, card, media, text};
use crate::wire::{array_at, at, bool_at, str_at, u64_at};

/// A tweet result after unwrapping X's wrappers.
pub(crate) enum Unwrapped<'a> {
    Tweet(&'a Value),
    Gone(UnavailableReason),
    Unknown(String),
}

/// Unwraps `tweet_results.result` (visibility wrappers, tombstones…).
pub(crate) fn unwrap(result: &Value) -> Unwrapped<'_> {
    match str_at(result, &["__typename"]) {
        Some("Tweet") => Unwrapped::Tweet(result),
        Some("TweetWithVisibilityResults") => match at(result, &["tweet"]) {
            Some(t) => Unwrapped::Tweet(t),
            None => Unwrapped::Unknown("visibility wrapper without tweet".into()),
        },
        Some("TweetTombstone") => Unwrapped::Gone(tombstone_reason(
            str_at(result, &["tombstone", "text", "text"]).unwrap_or_default(),
        )),
        Some("TweetUnavailable") => Unwrapped::Gone(match str_at(result, &["reason"]) {
            Some("Suspended") => UnavailableReason::Suspended,
            Some("Protected") => UnavailableReason::Protected,
            Some("Deleted") => UnavailableReason::Deleted,
            other => UnavailableReason::Other(other.map(str::to_owned)),
        }),
        // Some endpoints omit the typename on plain tweets.
        None if at(result, &["legacy"]).is_some() => Unwrapped::Tweet(result),
        other => Unwrapped::Unknown(format!("tweet result {}", other.unwrap_or("without type"))),
    }
}

fn tombstone_reason(text: &str) -> UnavailableReason {
    let t = text.to_lowercase();
    if t.contains("deleted") {
        UnavailableReason::Deleted
    } else if t.contains("suspended") {
        UnavailableReason::Suspended
    } else if t.contains("limits who can view") || t.contains("protected") {
        UnavailableReason::Protected
    } else if t.contains("withheld") {
        UnavailableReason::Withheld
    } else {
        UnavailableReason::Other((!text.is_empty()).then(|| text.to_owned()))
    }
}

/// A timeline row for a tweet result. `entry` is the timeline entry id.
pub(crate) fn item(result: &Value, entry: &str, ctx: &mut Ctx) -> FeedItem {
    let tweet = match unwrap(result) {
        Unwrapped::Tweet(t) => t,
        Unwrapped::Gone(reason) => {
            let id = str_at(result, &["rest_id"]).map(PostId::new);
            return FeedItem::Unavailable { entry: entry.to_owned(), id, reason };
        }
        Unwrapped::Unknown(reason) => return ctx.unsupported(entry, reason),
    };
    // A retweet is the original post, shown with "<author> reposted".
    if let Some(inner) = at(tweet, &["legacy", "retweeted_status_result", "result"]) {
        let by = match author(tweet, ctx) {
            Ok(by) => by,
            Err(e) => return ctx.unsupported(entry, e),
        };
        return match unwrap(inner) {
            Unwrapped::Tweet(t) => match post(t, ctx, 0) {
                Ok(post) => FeedItem::Repost { by, post },
                Err(e) => ctx.unsupported(entry, e),
            },
            Unwrapped::Gone(reason) => {
                FeedItem::Unavailable { entry: entry.to_owned(), id: None, reason }
            }
            Unwrapped::Unknown(e) => ctx.unsupported(entry, e),
        };
    }
    match post(tweet, ctx, 0) {
        Ok(p) => FeedItem::Post(p),
        Err(e) => ctx.unsupported(entry, e),
    }
}

fn author(tweet: &Value, ctx: &mut Ctx) -> Result<Arc<Profile>, String> {
    let user = at(tweet, &["core", "user_results", "result"]).ok_or("tweet without author")?;
    ctx.profile(user).ok_or_else(|| "author is not a usable user".to_owned())
}

/// Translates an unwrapped tweet. `depth` limits nested quotes to one level.
pub(crate) fn post(tweet: &Value, ctx: &mut Ctx, depth: u8) -> Result<Post, String> {
    let legacy = at(tweet, &["legacy"]).ok_or("tweet without legacy block")?;
    let id = str_at(tweet, &["rest_id"])
        .or_else(|| str_at(legacy, &["id_str"]))
        .ok_or("tweet without id")?;
    let author = author(tweet, ctx)?;
    let created_at = str_at(legacy, &["created_at"])
        .and_then(parse_date)
        .ok_or("tweet without valid created_at")?;

    let urls: card::UrlMap = array_at(legacy, &["entities", "urls"])
        .iter()
        .filter_map(|u| {
            Some((str_at(u, &["url"])?.to_owned(), str_at(u, &["expanded_url"])?.to_owned()))
        })
        .collect();

    let body = match at(tweet, &["note_tweet", "note_tweet_results", "result"]) {
        // Long posts: the full text lives in `note_tweet`, without a display range.
        Some(note) if str_at(note, &["text"]).is_some() => text::rich_text(
            str_at(note, &["text"]).unwrap_or_default(),
            at(note, &["entity_set"]).map(text::entities).unwrap_or_default(),
            None,
        ),
        _ => {
            let range = array_at(legacy, &["display_text_range"]);
            let range =
                match (range.first().and_then(Value::as_u64), range.get(1).and_then(Value::as_u64))
                {
                    (Some(a), Some(b)) => usize::try_from(a).ok().zip(usize::try_from(b).ok()),
                    _ => None,
                };
            text::rich_text(
                str_at(legacy, &["full_text"]).unwrap_or_default(),
                at(legacy, &["entities"]).map(text::entities).unwrap_or_default(),
                range,
            )
        }
    };

    let embed = match at(tweet, &["quoted_status_result", "result"]) {
        Some(q) if depth == 0 => Some(Embed::Quote { quoted: Box::new(quoted(q, ctx)) }),
        _ if depth == 0
            && bool_at(legacy, &["is_quote_status"]) == Some(true)
            && str_at(legacy, &["quoted_status_id_str"]).is_some() =>
        {
            // X omits the quoted post when it is gone or hidden from the viewer.
            Some(Embed::Quote {
                quoted: Box::new(PostOrGone::Unavailable {
                    reason: UnavailableReason::Other(None),
                }),
            })
        }
        _ => at(tweet, &["card"]).and_then(|c| card::embed(c, &urls)),
    };

    let reply_to = str_at(legacy, &["in_reply_to_status_id_str"]).map(|post| ReplyRef {
        post: Some(PostId::new(post)),
        user: str_at(legacy, &["in_reply_to_user_id_str"]).map(UserId::new),
        handle: str_at(legacy, &["in_reply_to_screen_name"]).map(Into::into),
    });

    let count = |k: &str| u64_at(legacy, &[k]).unwrap_or(0);
    let url = Url::parse(&format!("https://x.com/{}/status/{id}", author.handle))
        .map_err(|e| format!("cannot build post url: {e}"))?;
    Ok(Post {
        id: PostId::new(id),
        author,
        created_at,
        body,
        media: media::media(legacy),
        embed,
        reply_to,
        metrics: Metrics {
            replies: count("reply_count"),
            reposts: count("retweet_count"),
            quotes: count("quote_count"),
            likes: count("favorite_count"),
            bookmarks: count("bookmark_count"),
            views: u64_at(tweet, &["views", "count"]),
        },
        viewer: ViewerPostState {
            liked: bool_at(legacy, &["favorited"]).unwrap_or(false),
            reposted: bool_at(legacy, &["retweeted"]).unwrap_or(false),
            bookmarked: bool_at(legacy, &["bookmarked"]).unwrap_or(false),
        },
        lang: str_at(legacy, &["lang"]).filter(|l| *l != "und" && *l != "zxx").map(Into::into),
        url,
    })
}

fn quoted(result: &Value, ctx: &mut Ctx) -> PostOrGone {
    match unwrap(result) {
        Unwrapped::Tweet(t) => match post(t, ctx, 1) {
            Ok(p) => PostOrGone::Post(p),
            Err(e) => {
                ctx.warn(format!("quoted post: {e}"));
                PostOrGone::Unavailable { reason: UnavailableReason::Other(None) }
            }
        },
        Unwrapped::Gone(reason) => PostOrGone::Unavailable { reason },
        Unwrapped::Unknown(e) => {
            ctx.warn(format!("quoted post: {e}"));
            PostOrGone::Unavailable { reason: UnavailableReason::Other(None) }
        }
    }
}

/// X's date format: `Wed Oct 21 15:38:10 +0000 2026`.
pub(crate) fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y").ok().map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use twitty_model::Media;

    fn user(id: &str, handle: &str) -> Value {
        json!({"result": {"__typename": "User", "rest_id": id, "core": {"name": handle.to_uppercase(), "screen_name": handle}}})
    }

    fn tweet(id: &str, text: &str) -> Value {
        json!({
            "__typename": "Tweet",
            "rest_id": id,
            "core": {"user_results": user("1", "alice")},
            "views": {"count": "1234"},
            "legacy": {
                "created_at": "Wed Oct 21 15:38:10 +0000 2026",
                "full_text": text,
                "display_text_range": [0, text.chars().count()],
                "entities": {},
                "favorite_count": 5, "retweet_count": 2, "reply_count": 1, "quote_count": 0, "bookmark_count": 3,
                "favorited": true, "retweeted": false, "bookmarked": false, "lang": "en", "id_str": id
            }
        })
    }

    #[test]
    fn plain_post() {
        let mut ctx = Ctx::default();
        let FeedItem::Post(p) = item(&tweet("10", "hello"), "tweet-10", &mut ctx) else { panic!() };
        assert_eq!(p.id.as_str(), "10");
        assert_eq!(p.author.handle, "alice");
        assert_eq!(p.body.text(), "hello");
        assert_eq!(p.created_at.to_rfc3339(), "2026-10-21T15:38:10+00:00");
        assert_eq!(p.metrics.views, Some(1234));
        assert!(p.viewer.liked);
        assert_eq!(p.url.as_str(), "https://x.com/alice/status/10");
    }

    #[test]
    fn retweet_becomes_repost_of_original() {
        let mut outer = tweet("20", "RT @bob: original");
        let mut inner = tweet("19", "original");
        inner["core"]["user_results"] = user("2", "bob");
        outer["legacy"]["retweeted_status_result"] = json!({"result": inner});
        let FeedItem::Repost { by, post } = item(&outer, "tweet-20", &mut Ctx::default()) else {
            panic!()
        };
        assert_eq!(by.handle, "alice");
        assert_eq!(post.author.handle, "bob");
        assert_eq!(post.body.text(), "original");
    }

    #[test]
    fn visibility_wrapper_and_quote() {
        let mut t = tweet("30", "look at this");
        t["quoted_status_result"] = json!({"result": tweet("29", "quoted")});
        let wrapped = json!({"__typename": "TweetWithVisibilityResults", "tweet": t});
        let FeedItem::Post(p) = item(&wrapped, "tweet-30", &mut Ctx::default()) else { panic!() };
        let Some(Embed::Quote { quoted }) = p.embed else { panic!("quote") };
        assert!(matches!(*quoted, PostOrGone::Post(ref q) if q.body.text() == "quoted"));
    }

    #[test]
    fn missing_quoted_post_is_unavailable() {
        let mut t = tweet("31", "quoting a deleted post");
        t["legacy"]["is_quote_status"] = json!(true);
        t["legacy"]["quoted_status_id_str"] = json!("1");
        let FeedItem::Post(p) = item(&t, "tweet-31", &mut Ctx::default()) else { panic!() };
        assert!(
            matches!(p.embed, Some(Embed::Quote { quoted }) if matches!(*quoted, PostOrGone::Unavailable { .. }))
        );
    }

    #[test]
    fn note_tweet_text_wins_over_truncated_full_text() {
        let mut t = tweet("40", "truncated…");
        t["note_tweet"] = json!({"note_tweet_results": {"result": {"text": "the full long text #tag", "entity_set": {
            "hashtags": [{"text": "tag", "indices": [19, 23]}]
        }}}});
        let FeedItem::Post(p) = item(&t, "tweet-40", &mut Ctx::default()) else { panic!() };
        assert_eq!(p.body.text(), "the full long text #tag");
        assert_eq!(p.body.spans().len(), 1);
    }

    #[test]
    fn tombstones_and_unavailable() {
        let tomb = json!({"__typename": "TweetTombstone", "tombstone": {"text": {"text": "This Post was deleted by the Post author. Learn more"}}});
        assert!(matches!(
            item(&tomb, "tweet-1", &mut Ctx::default()),
            FeedItem::Unavailable { reason: UnavailableReason::Deleted, ref entry, .. } if entry == "tweet-1"
        ));
        let unav = json!({"__typename": "TweetUnavailable", "reason": "Suspended"});
        assert!(matches!(
            item(&unav, "tweet-2", &mut Ctx::default()),
            FeedItem::Unavailable { reason: UnavailableReason::Suspended, .. }
        ));
    }

    #[test]
    fn broken_tweets_become_unsupported_rows() {
        let mut ctx = Ctx::default();
        let mut t = tweet("50", "x");
        t["legacy"]["created_at"] = json!("yesterday");
        assert!(
            matches!(item(&t, "tweet-50", &mut ctx), FeedItem::Unsupported { ref entry, .. } if entry == "tweet-50")
        );
        assert!(matches!(
            item(&json!({"__typename": "Mystery"}), "e", &mut ctx),
            FeedItem::Unsupported { .. }
        ));
        assert_eq!(ctx.warnings.len(), 2);
    }

    #[test]
    fn reply_metadata_and_media() {
        let mut t = tweet("60", "@bob yes https://t.co/p");
        t["legacy"]["display_text_range"] = json!([5, 8]);
        t["legacy"]["in_reply_to_status_id_str"] = json!("59");
        t["legacy"]["in_reply_to_screen_name"] = json!("bob");
        t["legacy"]["extended_entities"] = json!({"media": [{"type": "photo", "media_url_https": "https://pbs.twimg.com/media/a.jpg"}]});
        let FeedItem::Post(p) = item(&t, "tweet-60", &mut Ctx::default()) else { panic!() };
        assert_eq!(p.body.text(), "yes");
        assert_eq!(p.reply_to.unwrap().handle.as_deref(), Some("bob"));
        assert!(matches!(p.media.as_slice(), [Media::Image { .. }]));
    }

    #[test]
    fn dates() {
        assert!(parse_date("Wed Oct 21 15:38:10 +0000 2026").is_some());
        assert!(parse_date("2026-10-21").is_none());
    }
}
