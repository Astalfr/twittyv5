//! Which GraphQL operation and variables serve each `FeedSource`.
//!
//! Every variable set here was verified live (see the table in `docs/x-api.md`);
//! a new source must be captured before it is mapped.

use serde_json::{Value, json};
use twitty_model::{
    Draft, FeedSource, FollowAction, HomeMode, NotificationFilter, PageRequest, PostAction, PostId,
    SearchSort, UserId, UserTab,
};

use crate::client::home_variables;

/// Rows asked for per page.
pub(crate) const PAGE_SIZE: u32 = 50;

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
        // Twitter only serves likes of the signed-in account; others' come back empty.
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

/// The mutation and variables for a viewer action, as the web client sends them.
pub(crate) fn action(action: &PostAction) -> (&'static str, Value) {
    let id = action.post_id().as_str();
    match action {
        PostAction::Like(_) => ("FavoriteTweet", json!({"tweet_id": id})),
        PostAction::Unlike(_) => ("UnfavoriteTweet", json!({"tweet_id": id})),
        PostAction::Repost(_) => ("CreateRetweet", json!({"tweet_id": id, "dark_request": false})),
        PostAction::Unrepost(_) => {
            ("DeleteRetweet", json!({"source_tweet_id": id, "dark_request": false}))
        }
        PostAction::Bookmark(_) => ("CreateBookmark", json!({"tweet_id": id})),
        PostAction::Unbookmark(_) => ("DeleteBookmark", json!({"tweet_id": id})),
    }
}

/// The v1.1 REST path and form fields for following or unfollowing `user`,
/// as the web client sends them.
pub(crate) fn follow(user: &UserId, action: FollowAction) -> (&'static str, Vec<(String, String)>) {
    let path = match action {
        FollowAction::Follow => "1.1/friendships/create.json",
        FollowAction::Unfollow => "1.1/friendships/destroy.json",
    };
    let fields = [
        ("include_profile_interstitial_type", "1"),
        ("include_blocking", "1"),
        ("include_blocked_by", "1"),
        ("include_followed_by", "1"),
        ("include_want_retweets", "1"),
        ("include_mute_edge", "1"),
        ("include_can_dm", "1"),
        ("include_can_media_tag", "1"),
        ("include_ext_is_blue_verified", "1"),
        ("include_ext_verified_type", "1"),
        ("include_ext_profile_image_shape", "1"),
        ("skip_status", "1"),
        ("user_id", user.as_str()),
    ];
    (path, fields.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
}

/// Variables of `CreateTweet` for a draft.
pub(crate) fn create_tweet(draft: &Draft) -> Value {
    let mut v = json!({
        "tweet_text": draft.text,
        "dark_request": false,
        "media": {"media_entities": [], "possibly_sensitive": false},
        "semantic_annotation_ids": [],
        "disallowed_reply_options": null,
    });
    if let Some(obj) = v.as_object_mut() {
        if let Some(parent) = &draft.reply_to {
            obj.insert(
                "reply".to_owned(),
                json!({"in_reply_to_tweet_id": parent.as_str(), "exclude_reply_user_ids": []}),
            );
        }
        if let Some(quoted) = &draft.quote {
            obj.insert(
                "attachment_url".to_owned(),
                json!(format!("https://x.com/i/status/{}", quoted.as_str())),
            );
        }
    }
    v
}

/// Variables of `SearchTimeline` for its "People" tab: accounts, not tweets.
pub(crate) fn search_users_variables(query: &str) -> Value {
    json!({
        "rawQuery": query,
        "count": PAGE_SIZE,
        "querySource": "typed_query",
        "product": "People",
    })
}

/// Variables of `ListsManagementPageTimeline` (the viewer's lists page). One
/// page of 100 holds every list but the rarest collections.
pub(crate) fn lists_management_variables() -> Value {
    json!({"count": 100})
}

/// Variables of `TweetResultsByRestIds` for tweets `ids` (verified live with
/// up to 90 ids).
pub(crate) fn tweet_results_variables(ids: &[PostId]) -> Value {
    json!({
        "tweetIds": ids.iter().map(PostId::as_str).collect::<Vec<_>>(),
        "includePromotedContent": true,
        "withBirdwatchNotes": true,
        "withVoice": true,
        "withCommunity": true,
    })
}

/// Variables of `UserByScreenName`.
pub(crate) fn user_by_screen_name_variables(handle: &str) -> Value {
    json!({"screen_name": handle.trim_start_matches('@')})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweet_lookups_send_the_ids_in_order() {
        let ids = [PostId::new("2"), PostId::new("1")];
        let vars = tweet_results_variables(&ids);
        assert_eq!(vars["tweetIds"], json!(["2", "1"]));
        assert_eq!(vars["withBirdwatchNotes"], json!(true));
    }

    #[test]
    fn follow_goes_to_friendships_with_the_user_id() {
        let user = UserId::new("42");
        let (path, fields) = follow(&user, FollowAction::Follow);
        assert_eq!(path, "1.1/friendships/create.json");
        assert!(fields.contains(&("user_id".into(), "42".into())));
        let (path, _) = follow(&user, FollowAction::Unfollow);
        assert_eq!(path, "1.1/friendships/destroy.json");
    }
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
            json!({"rawQuery": "rust lang:en", "count": 50, "querySource": "typed_query", "product": "Top"})
        );
        let (op, vars) = first(FeedSource::Notifications { filter: NotificationFilter::Mentions });
        assert_eq!(op, "NotificationsTimeline");
        assert_eq!(vars, json!({"timeline_type": "Mentions", "count": 50}));
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
        assert_eq!(vars, json!({"listId": "5", "count": 50}));
    }

    #[test]
    fn actions_use_the_web_mutations() {
        let id = PostId::new("42");
        let cases = [
            (PostAction::Like(id.clone()), "FavoriteTweet", json!({"tweet_id": "42"})),
            (PostAction::Unlike(id.clone()), "UnfavoriteTweet", json!({"tweet_id": "42"})),
            (
                PostAction::Repost(id.clone()),
                "CreateRetweet",
                json!({"tweet_id": "42", "dark_request": false}),
            ),
            (
                PostAction::Unrepost(id.clone()),
                "DeleteRetweet",
                json!({"source_tweet_id": "42", "dark_request": false}),
            ),
            (PostAction::Bookmark(id.clone()), "CreateBookmark", json!({"tweet_id": "42"})),
            (PostAction::Unbookmark(id), "DeleteBookmark", json!({"tweet_id": "42"})),
        ];
        for (a, op, vars) in cases {
            assert_eq!(action(&a), (op, vars));
        }
    }

    #[test]
    fn create_tweet_variables() {
        let plain = create_tweet(&Draft { text: "hello".into(), ..Draft::default() });
        assert_eq!(plain["tweet_text"], "hello");
        assert!(plain.get("reply").is_none() && plain.get("attachment_url").is_none());
        let reply = create_tweet(&Draft {
            text: "hi".into(),
            reply_to: Some(PostId::new("7")),
            quote: Some(PostId::new("8")),
        });
        assert_eq!(reply["reply"]["in_reply_to_tweet_id"], "7");
        assert_eq!(reply["attachment_url"], "https://x.com/i/status/8");
    }

    #[test]
    fn handles_lose_their_at_sign() {
        assert_eq!(user_by_screen_name_variables("@NASA"), json!({"screen_name": "NASA"}));
    }
}
