//! Which GraphQL operation and variables serve each `FeedSource`.
//!
//! Every variable set here was verified live (see the table in `docs/x-api.md`);
//! a new source must be captured before it is mapped.

use serde_json::{Value, json};
use twitty_model::{FeedSource, HomeMode, NotificationFilter, PageRequest, SearchSort, UserTab};

use crate::client::home_variables;

/// Rows asked for per page.
pub(crate) const PAGE_SIZE: u32 = 20;

/// The operation and variables for one page of `source`.
pub(crate) fn feed(source: &FeedSource, page: &PageRequest) -> (&'static str, Value) {
    let cursor = match page {
        PageRequest::First => None,
        PageRequest::Newer(t) | PageRequest::Older(t) => Some(t.as_str()),
    };
    let (operation, mut vars) = match source {
        FeedSource::Home { mode } => {
            let latest = *mode == HomeMode::Following;
            let op = if latest { "HomeLatestTimeline" } else { "HomeTimeline" };
            // `home_variables` already handles the cursor.
            return (op, home_variables(latest, PAGE_SIZE, cursor));
        }
        FeedSource::Notifications { filter } => {
            let timeline_type = match filter {
                NotificationFilter::All => "All",
                NotificationFilter::Verified => "Verified",
                NotificationFilter::Mentions => "Mentions",
            };
            ("NotificationsTimeline", json!({"timeline_type": timeline_type, "count": PAGE_SIZE}))
        }
        FeedSource::Search { query, sort } => {
            let product = match sort {
                SearchSort::Top => "Top",
                SearchSort::Latest => "Latest",
                SearchSort::Media => "Media",
            };
            (
                "SearchTimeline",
                json!({
                    "rawQuery": query,
                    "count": PAGE_SIZE,
                    "querySource": "typed_query",
                    "product": product,
                }),
            )
        }
        FeedSource::User { id, tab: UserTab::Posts, .. } => (
            "UserTweets",
            json!({
                "userId": id.as_str(),
                "count": PAGE_SIZE,
                "includePromotedContent": true,
                "withQuickPromoteEligibilityTweetFields": true,
                "withVoice": true,
            }),
        ),
        FeedSource::User { id, tab: UserTab::Replies, .. } => (
            "UserTweetsAndReplies",
            json!({
                "userId": id.as_str(),
                "count": PAGE_SIZE,
                "includePromotedContent": true,
                "withCommunity": true,
                "withVoice": true,
            }),
        ),
        // X only serves likes of the signed-in account; others' come back empty.
        FeedSource::User { id, tab: tab @ (UserTab::Media | UserTab::Likes), .. } => (
            if *tab == UserTab::Media { "UserMedia" } else { "Likes" },
            json!({
                "userId": id.as_str(),
                "count": PAGE_SIZE,
                "includePromotedContent": false,
                "withClientEventToken": false,
                "withBirdwatchNotes": false,
                "withVoice": true,
            }),
        ),
        FeedSource::List { id, .. } => {
            ("ListLatestTweetsTimeline", json!({"listId": id.as_str(), "count": PAGE_SIZE}))
        }
        FeedSource::Bookmarks => {
            ("Bookmarks", json!({"count": PAGE_SIZE, "includePromotedContent": true}))
        }
        FeedSource::Thread { root } => ("TweetDetail", tweet_detail_variables(root.as_str())),
    };
    if let (Some(c), Some(obj)) = (cursor, vars.as_object_mut()) {
        obj.insert("cursor".to_owned(), json!(c));
    }
    (operation, vars)
}

/// Variables of `TweetDetail` for the conversation around `focal`.
pub(crate) fn tweet_detail_variables(focal: &str) -> Value {
    json!({
        "focalTweetId": focal,
        "with_rux_injections": false,
        "rankingMode": "Relevance",
        "includePromotedContent": true,
        "withCommunity": true,
        "withQuickPromoteEligibilityTweetFields": true,
        "withBirdwatchNotes": true,
        "withVoice": true,
    })
}

/// Variables of `UserByScreenName`.
pub(crate) fn user_by_screen_name_variables(handle: &str) -> Value {
    json!({"screen_name": handle.trim_start_matches('@')})
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use twitty_model::{ListId, PageToken, PostId, UserId};

    fn first(source: FeedSource) -> (&'static str, Value) {
        feed(&source, &PageRequest::First)
    }

    #[test]
    fn home_modes_pick_their_timeline() {
        assert_eq!(first(FeedSource::Home { mode: HomeMode::ForYou }).0, "HomeTimeline");
        let (op, vars) = first(FeedSource::Home { mode: HomeMode::Following });
        assert_eq!(op, "HomeLatestTimeline");
        assert_eq!(vars["count"], PAGE_SIZE);
    }

    #[test]
    fn cursors_are_passed_in_both_directions() {
        let sources = [
            FeedSource::Home { mode: HomeMode::ForYou },
            FeedSource::Notifications { filter: NotificationFilter::All },
            FeedSource::Search { query: "rust".into(), sort: SearchSort::Latest },
            FeedSource::User { id: UserId::new("7"), handle: "x".into(), tab: UserTab::Posts },
            FeedSource::Bookmarks,
            FeedSource::Thread { root: PostId::new("9") },
            FeedSource::List { id: ListId::new("5"), name: None },
            FeedSource::User { id: UserId::new("7"), handle: "x".into(), tab: UserTab::Media },
        ];
        for source in &sources {
            for page in [
                PageRequest::Older(PageToken::new("BOTTOM")),
                PageRequest::Newer(PageToken::new("TOP")),
            ] {
                let (op, vars) = feed(source, &page);
                let expected = match &page {
                    PageRequest::Older(_) => "BOTTOM",
                    _ => "TOP",
                };
                assert_eq!(vars["cursor"], expected, "{op}");
            }
            let (op, vars) = feed(source, &PageRequest::First);
            assert!(vars.get("cursor").is_none(), "{op}");
        }
    }

    #[test]
    fn variables_match_the_live_verified_sets() {
        let (op, vars) =
            first(FeedSource::Search { query: "rust lang:en".into(), sort: SearchSort::Top });
        assert_eq!(op, "SearchTimeline");
        assert_eq!(
            vars,
            json!({"rawQuery": "rust lang:en", "count": 20, "querySource": "typed_query", "product": "Top"})
        );
        let (op, vars) = first(FeedSource::Notifications { filter: NotificationFilter::Mentions });
        assert_eq!(op, "NotificationsTimeline");
        assert_eq!(vars, json!({"timeline_type": "Mentions", "count": 20}));
        let (op, vars) = first(FeedSource::User {
            id: UserId::new("11348282"),
            handle: "NASA".into(),
            tab: UserTab::Posts,
        });
        assert_eq!(op, "UserTweets");
        assert_eq!(vars["userId"], "11348282");
        let (op, vars) = first(FeedSource::Thread { root: PostId::new("123") });
        assert_eq!(op, "TweetDetail");
        assert_eq!(vars["focalTweetId"], "123");
        assert_eq!(first(FeedSource::Bookmarks).0, "Bookmarks");
    }

    #[test]
    fn user_tabs_and_lists_use_their_operations() {
        let user = |tab| FeedSource::User { id: UserId::new("11348282"), handle: "a".into(), tab };
        let (op, vars) = first(user(UserTab::Replies));
        assert_eq!(op, "UserTweetsAndReplies");
        assert_eq!(vars["withCommunity"], true);
        let (op, vars) = first(user(UserTab::Media));
        assert_eq!(op, "UserMedia");
        assert_eq!(vars["userId"], "11348282");
        assert_eq!(first(user(UserTab::Likes)).0, "Likes");
        let (op, vars) = first(FeedSource::List { id: ListId::new("5"), name: None });
        assert_eq!(op, "ListLatestTweetsTimeline");
        assert_eq!(vars, json!({"listId": "5", "count": 20}));
    }

    #[test]
    fn handles_lose_their_at_sign() {
        assert_eq!(user_by_screen_name_variables("@NASA"), json!({"screen_name": "NASA"}));
    }
}
