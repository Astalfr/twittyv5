//! `TimelineNotification` items → `Notification`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use twitty_model::{FeedItem, Notification, NotificationId, NotificationKind, RichText};

use super::{Ctx, tweet};
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
        _ => NotificationKind::Other,
    };
    let actors = array_at(content, &["template", "from_users"])
        .iter()
        .filter_map(|u| at(u, &["user_results", "result"]))
        .filter_map(|u| ctx.profile(u))
        .collect();
    // The message is localized by X ("Alice liked your post", "Alice a aimé…").
    let text = RichText::plain(str_at(content, &["rich_message", "text"]).unwrap_or_default());
    let target = array_at(content, &["template", "target_objects"])
        .iter()
        .filter_map(|t| at(t, &["tweet_results", "result"]))
        .find_map(|r| match tweet::unwrap(r) {
            tweet::Unwrapped::Tweet(t) => tweet::post(t, ctx, 1).ok(),
            _ => None,
        })
        .map(Box::new);
    FeedItem::Notification(Notification {
        id: NotificationId::new(id),
        kind,
        created_at,
        actors,
        text,
        target,
    })
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

    #[test]
    fn broken_notification_is_unsupported() {
        let mut ctx = Ctx::default();
        assert!(matches!(item(&json!({"id": "x"}), "n", &mut ctx), FeedItem::Unsupported { .. }));
    }
}
