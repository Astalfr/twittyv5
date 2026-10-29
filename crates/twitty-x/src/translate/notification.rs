//! `TimelineNotification` items → `Notification`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use twitty_model::{
    CommunityNote, FeedItem, Notification, NotificationId, NotificationKind, RichText,
};
use url::Url;

use super::{Ctx, NoteLookup, text, tweet};
use crate::wire::{array_at, at, str_at};

pub(crate) fn item(content: &Value, entry: &str, ctx: &mut Ctx) -> FeedItem {
    let Some(id) = str_at(content, &["id"]) else {
        return ctx.unsupported(entry, "notification without id");
    };
    let Some(created_at) = str_at(content, &["timestamp_ms"])
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
    else {
        return ctx.unsupported(entry, "notification without valid timestamp");
    };
    let kind = match str_at(content, &["notification_icon"]) {
        Some("heart_icon") => NotificationKind::Like,
        Some("retweet_icon") => NotificationKind::Repost,
        Some("person_icon") => NotificationKind::Follow,
        Some("quote_icon" | "reply_icon") => NotificationKind::Quote,
        Some("birdwatch_icon") => NotificationKind::CommunityNote,
        _ => NotificationKind::Other,
    };
    let actors = array_at(content, &["template", "from_users"])
        .iter()
        .filter_map(|u| at(u, &["user_results", "result"]))
        .filter_map(|u| ctx.profile(u))
        .collect();
    // The message is localized by Twitter ("Alice liked your tweet", "Alice a aimé…").
    let text = RichText::plain(str_at(content, &["rich_message", "text"]).unwrap_or_default());
    let target = array_at(content, &["template", "target_objects"])
        .iter()
        .filter_map(|t| at(t, &["tweet_results", "result"]))
        .find_map(|r| match tweet::unwrap(r) {
            tweet::Unwrapped::Tweet(t) => tweet::post(t, ctx, 1).ok(),
            _ => None,
        })
        .map(Box::new);
    let id = NotificationId::new(id);
    let note = (kind == NotificationKind::CommunityNote).then(|| note(content, ctx)).flatten();
    if let Some(note) = note.as_ref().filter(|_| target.is_none()) {
        // Twitter names the note, not the tweet: the backend looks it up.
        match note_id(&note.url) {
            Some(note_id) => {
                ctx.notes.push(NoteLookup { notification: id.clone(), note: note_id.to_owned() })
            }
            None => ctx.warn(format!("{entry}: community note link without a note id")),
        }
    }
    FeedItem::Notification(Notification { id, kind, created_at, actors, text, target, note })
}

/// The note a Community Notes notification is about: its text is in
/// `template.additional_context` (code-point entities, like a tweet's
/// `birdwatch_pivot`), its page in `notification_url`.
fn note(content: &Value, ctx: &mut Ctx) -> Option<CommunityNote> {
    let context = at(content, &["template", "additional_context"]);
    let Some(raw) = context.and_then(|c| str_at(c, &["text"])).filter(|t| !t.trim().is_empty())
    else {
        ctx.warn("community note notification without the note's text".to_owned());
        return None;
    };
    let Some(url) = str_at(content, &["notification_url", "url"]).and_then(|u| Url::parse(u).ok())
    else {
        ctx.warn("community note notification without a valid link".to_owned());
        return None;
    };
    let ents =
        text::rich_message_entities(raw, context.map_or(&[], |c| array_at(c, &["entities"])));
    Some(CommunityNote { body: text::rich_text(raw, ents, None), url })
}

/// The note id in a note's page, `https://twitter.com/i/birdwatch/n/{id}`.
fn note_id(url: &Url) -> Option<&str> {
    let mut segments = url.path_segments()?;
    while let Some(segment) = segments.next() {
        if segment == "n" {
            return segments
                .next()
                .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn like_notification() {
        let content = json!({
            "__typename": "TimelineNotification",
            "id": "N1",
            "notification_icon": "heart_icon",
            "timestamp_ms": "2026-10-24T07:59:31.444Z",
            "rich_message": {"text": "Bob liked your post"},
            "template": {
                "from_users": [{"user_results": {"result": {"__typename": "User", "rest_id": "2", "core": {"name": "Bob", "screen_name": "bob"}}}}],
                "target_objects": [{"tweet_results": {"result": {"__typename": "Tweet", "rest_id": "9",
                    "core": {"user_results": {"result": {"__typename": "User", "rest_id": "1", "core": {"name": "Me", "screen_name": "me"}}}},
                    "legacy": {"created_at": "Wed Oct 21 15:38:10 +0000 2026", "full_text": "my post"}}}}]
            }
        });
        let FeedItem::Notification(n) = item(&content, "notification-N1", &mut Ctx::default())
        else {
            panic!()
        };
        assert_eq!(n.kind, NotificationKind::Like);
        assert_eq!(n.actors[0].handle, "bob");
        assert_eq!(n.text.text(), "Bob liked your post");
        assert_eq!(n.target.unwrap().body.text(), "my post");
    }

    fn note_notification(url: &str) -> Value {
        json!({
            "__typename": "TimelineNotification",
            "id": "N2",
            "notification_icon": "birdwatch_icon",
            "timestamp_ms": "2026-10-17T11:47:58.411Z",
            "notification_url": {"url": url, "urlType": "ExternalUrl"},
            "rich_message": {"text": "Readers added a Community Note to a post you liked"},
            "template": {
                "__typename": "TimelineNotificationAggregateUserActions",
                "additional_context": {
                    "text": "AI image 🦀 source: https://example.com/a",
                    "entities": [{"fromIndex": 19, "toIndex": 40,
                        "ref": {"type": "TimelineUrl", "url": "https://example.com/a"}}]
                },
                "from_users": [],
                "target_objects": []
            }
        })
    }

    #[test]
    fn community_note_notification_carries_the_note_and_asks_for_its_tweet() {
        let mut ctx = Ctx::default();
        let content = note_notification("https://twitter.com/i/birdwatch/n/42?src=ntab&source=10");
        let FeedItem::Notification(n) = item(&content, "notification-N2", &mut ctx) else {
            panic!()
        };
        assert_eq!(n.kind, NotificationKind::CommunityNote);
        assert!(n.actors.is_empty() && n.target.is_none());
        let note = n.note.expect("the note");
        assert_eq!(note.body.text(), "AI image 🦀 source: https://example.com/a");
        assert_eq!(note.body.spans().len(), 1, "its link, at code-point offsets");
        assert_eq!(note.url.path(), "/i/birdwatch/n/42");
        assert_eq!(ctx.notes, vec![NoteLookup { notification: n.id, note: "42".into() }]);
        assert!(ctx.warnings.is_empty(), "{:?}", ctx.warnings);
    }

    #[test]
    fn a_note_link_without_an_id_warns_and_keeps_the_note() {
        let mut ctx = Ctx::default();
        let content = note_notification("https://twitter.com/i/birdwatch");
        let FeedItem::Notification(n) = item(&content, "notification-N2", &mut ctx) else {
            panic!()
        };
        assert!(n.note.is_some());
        assert!(ctx.notes.is_empty());
        assert_eq!(ctx.warnings.len(), 1);
    }

    #[test]
    fn a_note_notification_without_text_has_no_note() {
        let mut ctx = Ctx::default();
        let mut content = note_notification("https://twitter.com/i/birdwatch/n/42");
        content["template"]["additional_context"] = json!(null);
        let FeedItem::Notification(n) = item(&content, "notification-N2", &mut ctx) else {
            panic!()
        };
        assert_eq!(n.kind, NotificationKind::CommunityNote);
        assert!(n.note.is_none() && ctx.notes.is_empty());
        assert_eq!(ctx.warnings.len(), 1);
    }

    #[test]
    fn broken_notification_is_unsupported() {
        let mut ctx = Ctx::default();
        assert!(matches!(item(&json!({"id": "x"}), "n", &mut ctx), FeedItem::Unsupported { .. }));
    }
}
