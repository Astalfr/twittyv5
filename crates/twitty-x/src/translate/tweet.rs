//! Tweet results → `Post` / `FeedItem`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use twitty_model::{
    CommunityNote, Embed, FeedItem, Metrics, Post, PostId, PostOrGone, Profile, ReplyRef,
    Translation, UnavailableReason, UserId, ViewerPostState,
};
use url::Url;

use super::{Ctx, card, media, text};
use crate::wire::{array_at, at, bool_at, str_at, u64_at};

/// A tweet result after unwrapping Twitter's wrappers.
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
    // A retweet is the original tweet, shown with "<author> retweeted".
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
        // Long tweets: the full text lives in `note_tweet`, without a display range.
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
            // Twitter omits the quoted tweet when it is gone or hidden from the viewer.
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
        translation: translation(tweet),
        community_note: community_note(tweet, ctx),
        url,
    })
}

/// Grok's automatic translation, sent when the
/// `responsive_web_grok_show_grok_translated_post` feature is on. It covers
/// the display text only (no reply mentions, no media link), with entity
/// offsets in code points like `legacy`, and for long tweets the whole note.
/// `preview_translation` is a truncated copy and is ignored.
fn translation(tweet: &Value) -> Option<Translation> {
    let grok = at(tweet, &["grok_translated_post_with_availability"])?;
    if bool_at(grok, &["is_available"]) != Some(true) {
        return None;
    }
    let data = at(grok, &["data"])?;
    let text = str_at(data, &["translation"]).filter(|t| !t.trim().is_empty())?;
    let from = str_at(data, &["source_language"]).filter(|l| !l.is_empty());
    let to = str_at(data, &["destination_language"]).filter(|l| !l.is_empty());
    if from.is_some() && from == to {
        return None;
    }
    let ents = at(data, &["entities"]).map(text::entities).unwrap_or_default();
    Some(Translation {
        body: text::rich_text(text, ents, None),
        from: from.map(Into::into),
        to: to.map(Into::into),
    })
}

/// The Community Note Twitter shows under a tweet, from `birdwatch_pivot`. Twitter only
/// sends the pivot for a note rated helpful, in timelines as well as in
/// `TweetDetail`; `has_birdwatch_notes` alone (notes still being rated) shows
/// nothing. The text's entities use code-point `fromIndex`/`toIndex`.
fn community_note(tweet: &Value, ctx: &mut Ctx) -> Option<CommunityNote> {
    let pivot = at(tweet, &["birdwatch_pivot"])?;
    let Some(raw) = str_at(pivot, &["subtitle", "text"]).filter(|t| !t.trim().is_empty()) else {
        ctx.warn("community note without text".to_owned());
        return None;
    };
    let Some(url) = str_at(pivot, &["destinationUrl"])
        .or_else(|| str_at(pivot, &["callToAction", "destinationUrl"]))
        .and_then(|u| Url::parse(u).ok())
    else {
        ctx.warn("community note without a valid link".to_owned());
        return None;
    };
    let ents = text::rich_message_entities(raw, array_at(pivot, &["subtitle", "entities"]));
    Some(CommunityNote { body: text::rich_text(raw, ents, None), url })
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

/// Twitter's date format: `Wed Oct 21 15:38:10 +0000 2026`.
pub(crate) fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y").ok().map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use twitty_model::{Media, SpanKind};

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

    fn grok(text: &str, entities: Value) -> Value {
        json!({"is_available": true, "data": {
            "associated_data": {},
            "source_language": "ja",
            "destination_language": "en",
            "entities": entities,
            "translation": text,
            "preview_translation": "truncated"
        }})
    }

    #[test]
    fn grok_translation_with_entities() {
        let mut t = tweet("70", "🦋こんにちは #挨拶 https://t.co/p");
        // Code-point offsets, after an astral emoji.
        t["grok_translated_post_with_availability"] = grok(
            "🦋Hello #Greeting https://t.co/a",
            json!({"hashtags": [{"text": "Greeting", "indices": [7, 16]}],
                   "urls": [{"url": "https://t.co/a", "display_url": "example.com/a",
                             "expanded_url": "https://example.com/a", "indices": [17, 31]}]}),
        );
        let FeedItem::Post(p) = item(&t, "tweet-70", &mut Ctx::default()) else { panic!() };
        let tr = p.translation.expect("translation");
        assert_eq!(tr.body.text(), "🦋Hello #Greeting example.com/a");
        assert_eq!(tr.from.as_deref(), Some("ja"));
        assert_eq!(tr.to.as_deref(), Some("en"));
        let spans: Vec<&str> =
            tr.body.spans().iter().map(|s| &tr.body.text()[s.range.clone()]).collect();
        assert_eq!(spans, ["#Greeting", "example.com/a"]);
        // The original stays the body.
        assert!(p.body.text().starts_with("🦋こんにちは"));
    }

    #[test]
    fn grok_translation_absent_unavailable_or_broken_is_none() {
        let cases = [
            json!({"is_available": false}),
            json!({"is_available": true}),
            json!({"is_available": true, "data": {"translation": "  "}}),
            json!({"is_available": true, "data": {"translation": 5}}),
            json!({"is_available": "yes", "data": {"translation": "hi"}}),
            json!("garbage"),
            json!({"is_available": true, "data": {"translation": "same", "source_language": "en", "destination_language": "en"}}),
        ];
        for case in cases {
            let mut t = tweet("71", "hello");
            t["grok_translated_post_with_availability"] = case.clone();
            let FeedItem::Post(p) = item(&t, "tweet-71", &mut Ctx::default()) else {
                panic!("{case}")
            };
            assert_eq!(p.translation, None, "{case}");
        }
        let FeedItem::Post(p) = item(&tweet("72", "hello"), "tweet-72", &mut Ctx::default()) else {
            panic!()
        };
        assert_eq!(p.translation, None);
    }

    #[test]
    fn grok_translation_out_of_range_entities_keep_the_text() {
        let mut t = tweet("73", "x");
        t["grok_translated_post_with_availability"] =
            grok("Hi", json!({"hashtags": [{"text": "far", "indices": [10, 14]}]}));
        let FeedItem::Post(p) = item(&t, "tweet-73", &mut Ctx::default()) else { panic!() };
        let tr = p.translation.expect("translation");
        assert_eq!(tr.body.text(), "Hi");
        assert!(tr.body.spans().is_empty());
    }

    #[test]
    fn a_quoted_post_keeps_its_translation() {
        let mut t = tweet("74", "look");
        let mut q = tweet("75", "日本語");
        q["grok_translated_post_with_availability"] = grok("Japanese", json!({}));
        t["quoted_status_result"] = json!({"result": q});
        let FeedItem::Post(p) = item(&t, "tweet-74", &mut Ctx::default()) else { panic!() };
        let Some(Embed::Quote { quoted }) = p.embed else { panic!("quote") };
        let PostOrGone::Post(q) = *quoted else { panic!("quoted post") };
        assert_eq!(q.translation.map(|t| t.body.text().to_owned()).as_deref(), Some("Japanese"));
    }

    fn pivot(text: &str) -> Value {
        json!({
            "destinationUrl": "https://twitter.com/i/birdwatch/n/9",
            "callToAction": {"destinationUrl": "https://twitter.com/i/birdwatch/n/9", "title": "Rate it"},
            "footer": {"text": "Context is written by people who use X.", "entities": []},
            "note": {"rest_id": "9", "language": "en"},
            "shorttitle": "Readers added context",
            "title": "Readers added context they thought people might want to know",
            "subtitle": {"text": text, "entities": [{
                "fromIndex": 12, "toIndex": 26,
                "ref": {"type": "TimelineUrl", "url": "https://t.co/a", "expandedUrl": "https://example.com/report"}
            }]}
        })
    }

    #[test]
    fn community_note_with_its_link() {
        let mut t = tweet("80", "a claim");
        t["birdwatch_pivot"] = pivot("Not true 🦀: example.com/r…");
        let mut ctx = Ctx::default();
        let FeedItem::Post(p) = item(&t, "tweet-80", &mut ctx) else { panic!() };
        let note = p.community_note.expect("note");
        assert_eq!(note.body.text(), "Not true 🦀: example.com/r…");
        assert_eq!(note.url.as_str(), "https://twitter.com/i/birdwatch/n/9");
        let link = &note.body.spans()[0];
        assert_eq!(&note.body.text()[link.range.clone()], "example.com/r…");
        assert!(
            matches!(&link.kind, SpanKind::Link { url } if url.as_str() == "https://example.com/report")
        );
        assert!(ctx.warnings.is_empty());
    }

    #[test]
    fn no_pivot_no_note_even_when_x_says_there_are_notes() {
        let mut t = tweet("81", "a claim");
        t["has_birdwatch_notes"] = json!(true);
        let FeedItem::Post(p) = item(&t, "tweet-81", &mut Ctx::default()) else { panic!() };
        assert_eq!(p.community_note, None);
    }

    #[test]
    fn a_broken_note_is_dropped_with_a_warning_but_the_post_stays() {
        let mut no_text = pivot("x");
        no_text["subtitle"] = json!({"entities": []});
        let mut bad_url = pivot("Not true 🦀: example.com/r…");
        bad_url["destinationUrl"] = json!("not a url");
        bad_url["callToAction"] = json!({});
        for broken in [no_text, bad_url, json!("garbage")] {
            let mut t = tweet("82", "a claim");
            t["birdwatch_pivot"] = broken.clone();
            let mut ctx = Ctx::default();
            let FeedItem::Post(p) = item(&t, "tweet-82", &mut ctx) else { panic!("{broken}") };
            assert_eq!(p.community_note, None, "{broken}");
            assert_eq!(p.body.text(), "a claim");
            assert_eq!(ctx.warnings.len(), 1, "{broken}");
        }
    }

    #[test]
    fn the_note_link_falls_back_to_the_call_to_action() {
        let mut t = tweet("83", "a claim");
        t["birdwatch_pivot"] = pivot("Not true 🦀: example.com/r…");
        t["birdwatch_pivot"]["destinationUrl"] = Value::Null;
        let FeedItem::Post(p) = item(&t, "tweet-83", &mut Ctx::default()) else { panic!() };
        assert_eq!(
            p.community_note.map(|n| n.url.to_string()).as_deref(),
            Some("https://twitter.com/i/birdwatch/n/9")
        );
    }

    #[test]
    fn dates() {
        assert!(parse_date("Wed Oct 21 15:38:10 +0000 2026").is_some());
        assert!(parse_date("2026-10-21").is_none());
    }
}
