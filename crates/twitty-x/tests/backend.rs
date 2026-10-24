//! `XBackend` end to end against a mock server: the right operation and
//! variables per source, cursors passed through, and responses (the committed
//! anonymised fixtures) translated into `twitty-model`.

#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] fns")]

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use twitty_model::{
    Backend, BackendError, FeedItem, FeedSource, HomeMode, NotificationFilter, PageRequest,
    PageToken, PostAction, PostId, SearchSort, UserId, UserTab,
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
async fn unmapped_sources_fail_without_a_request() {
    let server = MockServer::start().await;
    let source = FeedSource::User { id: UserId::new("1"), handle: "a".into(), tab: UserTab::Likes };
    let err = backend(&server).feed(&source, PageRequest::First).await.unwrap_err();
    assert!(matches!(err, BackendError::Upstream(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
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
    // Turn the capture into a reply: put the first reply's tweet above the focal post.
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
async fn writes_are_refused_without_touching_the_account() {
    let server = MockServer::start().await;
    let b = backend(&server);
    assert!(b.act(PostAction::Like(PostId::new("1"))).await.is_err());
    assert!(b.publish(twitty_model::Draft::default()).await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}
