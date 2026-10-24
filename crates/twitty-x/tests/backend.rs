//! `XBackend` end to end against a mock server: the right operation and
//! variables per source, cursors passed through, and responses (the committed
//! anonymised fixtures) translated into `twitty-model`.

#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] fns")]

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use twitty_model::{
    Backend, BackendError, FeedItem, FeedSource, FollowAction, FollowStatus, HomeMode, ListId,
    NotificationFilter, PageRequest, PageToken, PostAction, PostId, SearchSort, UserId, UserTab,
};
use twitty_x::{
    Session, XBackend, XClient,
    catalog::Catalog,
    transport::{ClientIds, RetryPolicy, Transport},
};
use url::Url;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path_regex},
};

fn backend(server: &MockServer) -> XBackend {
    let base = Url::parse(&format!("{}/i/api/", server.uri())).unwrap();
    let retry = RetryPolicy { max_retries: 0, base_delay: Duration::from_millis(1) };
    let session = Session::new("token", "csrf").unwrap();
    let transport = Transport::new(session, &ClientIds::random(), base, retry).unwrap();
    XBackend::new(Arc::new(XClient::new(transport, Arc::new(Catalog::new(None)), None)))
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Serves `body` for every GraphQL call to `operation`.
async fn serve(server: &MockServer, operation: &str, body: Value) {
    Mock::given(path_regex(format!("^/i/api/graphql/[^/]+/{operation}$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// The variables of the only request the server received.
async fn sent_variables(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    variables_of(&requests[0])
}

fn variables_of(req: &Request) -> Value {
    match req.url.query_pairs().find(|(k, _)| k == "variables") {
        Some((_, v)) => serde_json::from_str(&v).unwrap(),
        None => serde_json::from_slice::<Value>(&req.body).unwrap()["variables"].clone(),
    }
}

#[tokio::test]
async fn home_is_translated_from_the_live_capture() {
    let server = MockServer::start().await;
    serve(&server, "HomeTimeline", fixture("home")).await;
    let page = backend(&server)
        .feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 28, "same rows as the translate snapshot");
    assert!(page.older.is_some() && page.newer.is_some());
}

#[tokio::test]
async fn older_and_newer_send_the_page_tokens() {
    for (request, expected) in [
        (PageRequest::Older(PageToken::new("BOTTOM-1")), "BOTTOM-1"),
        (PageRequest::Newer(PageToken::new("TOP-1")), "TOP-1"),
    ] {
        let server = MockServer::start().await;
        serve(&server, "HomeLatestTimeline", fixture("home--latest")).await;
        backend(&server)
            .feed(&FeedSource::Home { mode: HomeMode::Following }, request)
            .await
            .unwrap();
        assert_eq!(sent_variables(&server).await["cursor"], expected);
    }
}

#[tokio::test]
async fn each_source_calls_its_operation() {
    let cases = [
        (
            FeedSource::Search { query: "rust".into(), sort: SearchSort::Latest },
            "SearchTimeline",
            "search-rust",
        ),
        (
            FeedSource::User {
                id: UserId::new("11348282"),
                handle: "NASA".into(),
                tab: UserTab::Posts,
            },
            "UserTweets",
            "usertweets-nasa",
        ),
        (
            FeedSource::Notifications { filter: NotificationFilter::All },
            "NotificationsTimeline",
            "notifications",
        ),
        (
            FeedSource::Thread { root: PostId::new("8184262359040044601") },
            "TweetDetail",
            "tweetdetail",
        ),
        (
            FeedSource::User {
                id: UserId::new("11348282"),
                handle: "NASA".into(),
                tab: UserTab::Replies,
            },
            "UserTweetsAndReplies",
            "userreplies-nasa",
        ),
        (
            FeedSource::List { id: ListId::new("1613023332353318913"), name: None },
            "ListLatestTweetsTimeline",
            "list-space",
        ),
    ];
    for (source, operation, name) in cases {
        let server = MockServer::start().await;
        serve(&server, operation, fixture(name)).await;
        let page = backend(&server).feed(&source, PageRequest::First).await.unwrap();
        assert!(!page.items.is_empty(), "{operation}");
        assert!(
            page.items.iter().all(|i| !matches!(i, FeedItem::Unsupported { .. })),
            "{operation}"
        );
    }
}

#[tokio::test]
async fn search_is_sent_as_post() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex("/SearchTimeline$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("search-rust")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("/SearchTimeline$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let source = FeedSource::Search { query: "rust lang:en".into(), sort: SearchSort::Top };
    let page = backend(&server).feed(&source, PageRequest::First).await.unwrap();
    assert!(!page.items.is_empty());
    let requests = server.received_requests().await.unwrap();
    let post = requests.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
    assert_eq!(variables_of(post)["product"], "Top");
}

#[tokio::test]
async fn unauthorized_maps_to_backend_error() {
    let server = MockServer::start().await;
    Mock::given(path_regex("/HomeTimeline$"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"errors": [{"code": 32, "message": "Could not authenticate you."}]}),
        ))
        .mount(&server)
        .await;
    let err = backend(&server)
        .feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First)
        .await
        .unwrap_err();
    assert_eq!(err, BackendError::Unauthorized);
}

#[tokio::test]
async fn media_column_pages_through_the_grid() {
    let source = FeedSource::User {
        id: UserId::new("11348282"),
        handle: "NASA".into(),
        tab: UserTab::Media,
    };
    let server = MockServer::start().await;
    serve(&server, "UserMedia", fixture("usermedia-nasa")).await;
    let first = backend(&server).feed(&source, PageRequest::First).await.unwrap();
    let server = MockServer::start().await;
    serve(&server, "UserMedia", fixture("usermedia-nasa-2")).await;
    let older = first.older.clone().unwrap();
    let second = backend(&server).feed(&source, PageRequest::Older(older)).await.unwrap();
    assert!(!first.items.is_empty() && !second.items.is_empty(), "page 2 comes as AddToModule");
}

#[tokio::test]
async fn thread_splits_around_the_focal_post() {
    let server = MockServer::start().await;
    serve(&server, "TweetDetail", fixture("tweetdetail")).await;
    let focal = PostId::new("8184262359040044601");
    let thread = backend(&server).thread(&focal).await.unwrap();
    assert_eq!(thread.focus.id, focal);
    assert!(thread.ancestors.is_empty(), "the captured post is not a reply");
    assert!(!thread.replies.is_empty());
    assert_eq!(sent_variables(&server).await["focalTweetId"], "8184262359040044601");
}

#[tokio::test]
async fn thread_rows_before_the_focal_post_are_ancestors() {
    // Turn the capture into a reply: put the first reply's tweet above the focal tweet.
    let mut body = fixture("tweetdetail");
    let entries = body["data"]["threaded_conversation_with_injections_v2"]["instructions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find_map(|i| i.get_mut("entries"))
        .unwrap()
        .as_array_mut()
        .unwrap();
    let reply = entries
        .iter()
        .find(|e| e["entryId"].as_str().unwrap().starts_with("conversationthread-"))
        .unwrap()["content"]["items"][0]["item"]["itemContent"]
        .clone();
    let parent_id = reply["tweet_results"]["result"]["rest_id"].as_str().unwrap().to_owned();
    entries.insert(
        0,
        json!({"entryId": format!("tweet-{parent_id}"), "content": {
            "entryType": "TimelineTimelineItem", "itemContent": reply
        }}),
    );
    let server = MockServer::start().await;
    serve(&server, "TweetDetail", body).await;
    let focal = PostId::new("8184262359040044601");
    let thread = backend(&server).thread(&focal).await.unwrap();
    let ancestors: Vec<&str> = thread.ancestors.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ancestors, [parent_id.as_str()]);
    assert_eq!(thread.focus.id, focal);
}

#[tokio::test]
async fn thread_of_a_missing_post_is_not_found() {
    let server = MockServer::start().await;
    serve(&server, "TweetDetail", fixture("tweetdetail")).await;
    let err = backend(&server).thread(&PostId::new("1")).await.unwrap_err();
    assert_eq!(err, BackendError::NotFound);
}

/// Shape of a live `Viewer` response (2026-10-24), trimmed to the fields
/// Twitty reads; the values are made up.
fn viewer_body() -> Value {
    json!({"data": {"viewer": {
        "is_tfe_restricted_session": false,
        "user_results": {"result": {
            "__typename": "User",
            "id": "VXNlcjo0Mg==",
            "rest_id": "42",
            "core": {"created_at": "Mon Jan 01 00:00:00 +0000 2018", "name": "Test Viewer", "screen_name": "test_viewer"},
            "avatar": {"image_url": "https://pbs.twimg.com/profile_images/1/a_normal.jpg"},
            "is_blue_verified": false,
            "privacy": {"protected": false},
            "profile_bio": {"description": "hello"},
            "relationship_counts": {"followers": 3, "following": 4},
            "tweet_counts": {"tweets": 5},
            "verification": {"verified": false}
        }}
    }}})
}

#[tokio::test]
async fn viewer_reads_the_signed_in_account() {
    let server = MockServer::start().await;
    serve(&server, "Viewer", viewer_body()).await;
    let me = backend(&server).viewer().await.unwrap();
    assert_eq!((me.id.as_str(), me.handle.as_str()), ("42", "test_viewer"));
    assert_eq!(me.counts.followers, 3);
}

#[tokio::test]
async fn viewer_without_user_is_a_decode_error() {
    let server = MockServer::start().await;
    serve(&server, "Viewer", json!({"data": {"viewer": {}}})).await;
    assert!(matches!(backend(&server).viewer().await, Err(BackendError::Decode(_))));
}

#[tokio::test]
async fn profile_looks_up_by_handle() {
    let server = MockServer::start().await;
    let user = viewer_body()["data"]["viewer"]["user_results"]["result"].clone();
    serve(&server, "UserByScreenName", json!({"data": {"user": {"result": user}}})).await;
    let p = backend(&server).profile("@test_viewer").await.unwrap();
    assert_eq!(p.handle, "test_viewer");
    assert_eq!(sent_variables(&server).await, json!({"screen_name": "test_viewer"}));
}

#[tokio::test]
async fn people_search_asks_for_the_people_tab_and_lists_accounts() {
    let server = MockServer::start().await;
    serve(&server, "SearchTimeline", fixture("search-people")).await;
    let b = backend(&server);
    let users = b.search_users("rust").await.unwrap();
    assert_eq!(users.len(), 20, "same accounts as the translate snapshot");
    let vars = sent_variables(&server).await;
    assert_eq!(vars["rawQuery"], "rust");
    assert_eq!(vars["product"], "People");
    let health = b.health();
    let people = health.iter().find(|h| h.name == "SearchTimeline (People)").unwrap();
    assert_eq!((people.ok, people.rows, people.unsupported), (1, 20, 0));
    assert!(health.iter().all(|h| h.name != "SearchTimeline"), "apart from search columns");
}

#[tokio::test]
async fn an_unreadable_account_is_skipped_and_counted() {
    let server = MockServer::start().await;
    let mut body = fixture("search-people");
    let instructions =
        &mut body["data"]["search_by_raw_query"]["search_timeline"]["timeline"]["instructions"];
    let entries =
        instructions.as_array_mut().unwrap().iter_mut().find_map(|i| i.get_mut("entries")).unwrap();
    entries[0]["content"]["itemContent"]["user_results"]["result"] =
        json!({"__typename": "UserUnavailable"});
    serve(&server, "SearchTimeline", body).await;
    let b = backend(&server);
    assert_eq!(b.search_users("rust").await.unwrap().len(), 19);
    let people = b.health().into_iter().find(|h| h.name == "SearchTimeline (People)").unwrap();
    assert_eq!((people.rows, people.unsupported, people.warnings), (20, 1, 1));
}

/// The body of the only request, which must be a POST.
async fn sent_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    assert_eq!(requests[0].method, wiremock::http::Method::POST, "mutations are POSTs");
    serde_json::from_slice(&requests[0].body).unwrap()
}

#[tokio::test]
async fn actions_post_their_mutation() {
    let cases = [
        (PostAction::Like(PostId::new("1")), "FavoriteTweet", "tweet_id"),
        (PostAction::Unlike(PostId::new("1")), "UnfavoriteTweet", "tweet_id"),
        (PostAction::Repost(PostId::new("1")), "CreateRetweet", "tweet_id"),
        (PostAction::Unrepost(PostId::new("1")), "DeleteRetweet", "source_tweet_id"),
        (PostAction::Bookmark(PostId::new("1")), "CreateBookmark", "tweet_id"),
        (PostAction::Unbookmark(PostId::new("1")), "DeleteBookmark", "tweet_id"),
    ];
    for (action, operation, id_field) in cases {
        let server = MockServer::start().await;
        serve(&server, operation, json!({"data": {"result": "Done"}})).await;
        backend(&server).act(action).await.unwrap();
        let body = sent_body(&server).await;
        assert_eq!(body["variables"][id_field], "1", "{operation}");
        assert!(body["queryId"].is_string(), "{operation}");
    }
}

#[tokio::test]
async fn rate_budget_uses_the_limit_x_reported() {
    let server = MockServer::start().await;
    Mock::given(path_regex("/HomeTimeline$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-rate-limit-limit", "500")
                .insert_header("x-rate-limit-remaining", "499")
                .insert_header("x-rate-limit-reset", "4102444800")
                .set_body_json(fixture("home")),
        )
        .mount(&server)
        .await;
    let b = backend(&server);
    let home = FeedSource::Home { mode: HomeMode::ForYou };
    let assumed = b.rate_budget(&home).unwrap();
    assert_eq!(assumed.limit, 50, "conservative before X answered");
    b.feed(&home, PageRequest::First).await.unwrap();
    let known = b.rate_budget(&home).unwrap();
    assert_eq!((known.limit, known.window.as_secs()), (500, 900));
}

#[tokio::test]
async fn health_counts_rows_errors_and_drift() {
    let server = MockServer::start().await;
    serve(&server, "HomeTimeline", fixture("home")).await;
    // A timeline whose entries Twitter renamed: one unknown item type.
    serve(
        &server,
        "Bookmarks",
        json!({"data": {"bookmark_timeline_v2": {"timeline": {"instructions": [
            {"type": "TimelineAddEntries", "entries": [
                {"entryId": "x-1", "content": {"entryType": "TimelineTimelineItem",
                    "itemContent": {"__typename": "TimelineBrandNewThing"}}}
            ]}
        ]}}}}),
    )
    .await;
    Mock::given(path_regex("/NotificationsTimeline$"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let b = backend(&server);
    b.feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First).await.unwrap();
    b.feed(&FeedSource::Bookmarks, PageRequest::First).await.unwrap();
    let notifications = FeedSource::Notifications { filter: NotificationFilter::All };
    assert!(b.feed(&notifications, PageRequest::First).await.is_err());

    let health = b.health();
    let get = |name: &str| health.iter().find(|h| h.name == name).cloned().unwrap();
    let home = get("HomeTimeline");
    assert_eq!((home.ok, home.rows, home.unsupported, home.errors), (1, 28, 0, 0));
    let bookmarks = get("Bookmarks");
    assert_eq!((bookmarks.unsupported, bookmarks.warnings), (1, 1));
    assert!(bookmarks.last_warning.unwrap().contains("TimelineBrandNewThing"));
    let notif = get("NotificationsTimeline");
    assert_eq!(notif.errors, 1);
    assert!(notif.last_error.unwrap().contains("500"));
}

#[tokio::test]
async fn a_response_without_a_timeline_is_flagged() {
    let server = MockServer::start().await;
    serve(&server, "Bookmarks", json!({"data": {"something_else": {}}})).await;
    let b = backend(&server);
    let page = b.feed(&FeedSource::Bookmarks, PageRequest::First).await.unwrap();
    assert!(page.items.is_empty());
    let h = b.health().into_iter().find(|h| h.name == "Bookmarks").unwrap();
    assert_eq!(h.warnings, 1);
}

#[tokio::test]
async fn already_done_actions_succeed() {
    for body in [
        json!({"errors": [{"code": 139, "message": "You have already favorited this status."}]}),
        json!({"data": {}, "errors": [{"code": 139, "message": "already"}]}),
        json!({"errors": [{"code": 327, "message": "You have already retweeted this Tweet."}]}),
    ] {
        let server = MockServer::start().await;
        serve(&server, "FavoriteTweet", body).await;
        backend(&server).act(PostAction::Like(PostId::new("1"))).await.unwrap();
    }
}

#[tokio::test]
async fn failed_actions_report_xs_message() {
    let server = MockServer::start().await;
    serve(
        &server,
        "CreateBookmark",
        json!({"data": {}, "errors": [{"code": 999, "message": "nope"}]}),
    )
    .await;
    let err = backend(&server).act(PostAction::Bookmark(PostId::new("1"))).await.unwrap_err();
    assert_eq!(err, BackendError::Upstream("Twitter error 999: nope".into()));
}

/// Serves `status` and `body` for every POST to the v1.1 REST `path`.
async fn serve_rest(server: &MockServer, path: &str, status: u16, body: Value) {
    Mock::given(method("POST"))
        .and(wiremock::matchers::path(format!("/i/api/1.1/{path}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

/// The form fields of the only request.
async fn sent_form(server: &MockServer) -> Vec<(String, String)> {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    url::form_urlencoded::parse(&requests[0].body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

#[tokio::test]
async fn following_posts_the_user_id_to_friendships() {
    let user = UserId::new("1000000000000000001");
    let cases = [
        (
            FollowAction::Follow,
            "friendships/create.json",
            "friendships-create",
            FollowStatus::Following,
        ),
        (
            FollowAction::Follow,
            "friendships/create.json",
            "friendships-create-protected",
            FollowStatus::Requested,
        ),
        (
            FollowAction::Unfollow,
            "friendships/destroy.json",
            "friendships-create",
            FollowStatus::NotFollowing,
        ),
    ];
    for (action, path, body, expected) in cases {
        let server = MockServer::start().await;
        serve_rest(&server, path, 200, fixture(body)).await;
        let status = backend(&server).set_following(&user, action).await.unwrap();
        assert_eq!(status, expected, "{action:?} with {body}");
        let form = sent_form(&server).await;
        assert!(form.contains(&("user_id".into(), user.as_str().into())), "{form:?}");
    }
}

#[tokio::test]
async fn a_pending_request_counts_as_requested() {
    let server = MockServer::start().await;
    let body = json!({"errors": [{"code": 160, "message": "You've already requested to follow."}]});
    serve_rest(&server, "friendships/create.json", 403, body).await;
    let status =
        backend(&server).set_following(&UserId::new("2"), FollowAction::Follow).await.unwrap();
    assert_eq!(status, FollowStatus::Requested);
}

#[tokio::test]
async fn a_refused_follow_is_an_error_not_a_lost_session() {
    let server = MockServer::start().await;
    let body = json!({"errors": [{"code": 162, "message": "You have been blocked from following this account."}]});
    serve_rest(&server, "friendships/create.json", 403, body).await;
    let err =
        backend(&server).set_following(&UserId::new("3"), FollowAction::Follow).await.unwrap_err();
    assert!(matches!(&err, BackendError::Upstream(m) if m.contains("blocked")), "{err:?}");
}

#[tokio::test]
async fn publish_sends_the_draft_and_returns_the_post() {
    // CreateTweet answers with the new tweet; reuse a captured one.
    let detail = fixture("tweetdetail");
    let tweet = detail["data"]["threaded_conversation_with_injections_v2"]["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|i| i.get("entries"))
        .unwrap()[0]["content"]["itemContent"]["tweet_results"]
        .clone();
    let server = MockServer::start().await;
    serve(&server, "CreateTweet", json!({"data": {"create_tweet": {"tweet_results": tweet}}}))
        .await;
    let draft =
        twitty_model::Draft { text: "hello".into(), reply_to: Some(PostId::new("7")), quote: None };
    let post = backend(&server).publish(draft).await.unwrap();
    assert_eq!(post.id.as_str(), "8184262359040044601");
    let body = sent_body(&server).await;
    assert_eq!(body["variables"]["tweet_text"], "hello");
    assert_eq!(body["variables"]["reply"]["in_reply_to_tweet_id"], "7");
}

#[tokio::test]
async fn rejected_posts_are_errors() {
    let server = MockServer::start().await;
    serve(
        &server,
        "CreateTweet",
        json!({"errors": [{"code": 187, "message": "Status is a duplicate."}]}),
    )
    .await;
    let err = backend(&server).publish(twitty_model::Draft::default()).await.unwrap_err();
    assert!(matches!(err, BackendError::Upstream(m) if m.contains("duplicate")));
}
