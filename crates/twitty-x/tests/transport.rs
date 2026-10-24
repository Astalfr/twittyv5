//! End-to-end tests of the X client against a mock server (no real network).

#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] fns")]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use twitty_x::{
    Session, XClient, XError,
    catalog::Catalog,
    discovery::Discovery,
    transport::{ClientIds, RetryPolicy, Transport},
};
use url::Url;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{header, method, path},
};

const TOKEN: &str = "secret-auth-token-value";
const CSRF: &str = "secret-ct0-value";

fn session() -> Session {
    Session::new(TOKEN, CSRF).unwrap()
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy { max_retries: 2, base_delay: Duration::from_millis(1) }
}

fn transport(server: &MockServer) -> Transport {
    let base = Url::parse(&format!("{}/i/api/", server.uri())).unwrap();
    Transport::new(session(), &ClientIds::random(), base, fast_retry()).unwrap()
}

fn client(server: &MockServer) -> XClient {
    XClient::new(transport(server), Arc::new(Catalog::new(None)), None)
}

fn client_with_discovery(server: &MockServer) -> XClient {
    let page = Url::parse(&format!("{}/home", server.uri())).unwrap();
    XClient::new(
        transport(server),
        Arc::new(Catalog::new(None)),
        Some(Discovery::new(vec![page]).unwrap()),
    )
}

const HOME_PATH: &str = "/i/api/graphql/0toR0MxC06yY3Fc57GXBpA/HomeTimeline";

fn ok_body() -> Value {
    json!({"data": {"home": {"home_timeline_urt": {"instructions": []}}}})
}

#[tokio::test]
async fn sends_auth_headers_and_graphql_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(HOME_PATH))
        .and(header("x-csrf-token", CSRF))
        .and(header("cookie", format!("auth_token={TOKEN}; ct0={CSRF}").as_str()))
        .and(header("x-twitter-auth-type", "OAuth2Session"))
        .and(header("x-twitter-active-user", "yes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;

    let body = client(&server).home_timeline(false, 20, None).await.unwrap();
    assert_eq!(body, ok_body());

    let req = &server.received_requests().await.unwrap()[0];
    let q: std::collections::HashMap<_, _> = req.url.query_pairs().into_owned().collect();
    let vars: Value = serde_json::from_str(&q["variables"]).unwrap();
    assert_eq!(vars["count"], 20);
    // Only the feature switches HomeTimeline declares are sent.
    let features: Value = serde_json::from_str(&q["features"]).unwrap();
    let declared = Catalog::new(None).resolve("HomeTimeline").unwrap().0.feature_switches.unwrap();
    assert_eq!(features.as_object().unwrap().len(), declared.len());
    assert!(q.contains_key("fieldToggles") || declared.is_empty());
    assert!(req.headers["authorization"].to_str().unwrap().starts_with("Bearer AAAA"));
    let txid = req.headers["x-client-transaction-id"].to_str().unwrap();
    assert_eq!(txid.len(), 32);
}

#[tokio::test]
async fn each_request_gets_a_fresh_transaction_id() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let c = client(&server);
    c.home_timeline(false, 20, None).await.unwrap();
    c.home_timeline(false, 20, None).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_ne!(
        reqs[0].headers["x-client-transaction-id"],
        reqs[1].headers["x-client-transaction-id"]
    );
}

#[tokio::test]
async fn rotated_cookies_are_used_next_and_reported() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH))
        .and(header("x-csrf-token", CSRF))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .append_header("set-cookie", "ct0=rotated-csrf; Path=/; Secure")
                .append_header("set-cookie", "guest_id=v1; Path=/"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path(HOME_PATH))
        .and(header("x-csrf-token", "rotated-csrf"))
        .and(header("cookie", format!("auth_token={TOKEN}; ct0=rotated-csrf").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;

    let seen: Arc<Mutex<Vec<Session>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let base = Url::parse(&format!("{}/i/api/", server.uri())).unwrap();
    let transport = Transport::new(session(), &ClientIds::random(), base, fast_retry())
        .unwrap()
        .on_rotate(Arc::new(move |s: &Session| sink.lock().unwrap().push(s.clone())));
    let c = XClient::new(transport, Arc::new(Catalog::new(None)), None);

    c.home_timeline(false, 20, None).await.unwrap();
    c.home_timeline(false, 20, None).await.unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "hook fires once per actual change");
    assert_eq!(seen[0].ct0.expose(), "rotated-csrf");
    assert_eq!(seen[0].auth_token.expose(), TOKEN);
}

#[tokio::test]
async fn rate_limit_429_uses_reset_header_and_blocks_locally() {
    let server = MockServer::start().await;
    let reset = chrono::Utc::now().timestamp() + 600;
    Mock::given(path(HOME_PATH))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("x-rate-limit-limit", "500")
                .append_header("x-rate-limit-remaining", "0")
                .append_header("x-rate-limit-reset", reset.to_string().as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server);

    let err = c.home_timeline(false, 20, None).await.unwrap_err();
    assert!(
        matches!(&err, XError::RateLimited { operation, reset_at } if operation == "HomeTimeline" && reset_at.timestamp() == reset),
        "{err:?}"
    );
    // The second call must not reach the server (expect(1) above).
    assert!(matches!(c.home_timeline(false, 20, None).await, Err(XError::RateLimited { .. })));
    // Other operations have their own budget.
    assert!(c.transport().limits().check("SearchTimeline", chrono::Utc::now()).is_ok());
}

#[tokio::test]
async fn server_errors_are_retried() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(path(HOME_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    assert!(client(&server).home_timeline(false, 20, None).await.is_ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn retries_give_up_after_policy() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH)).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let err = client(&server).home_timeline(false, 20, None).await.unwrap_err();
    assert!(matches!(err, XError::Http { status: 500, .. }));
    assert_eq!(server.received_requests().await.unwrap().len(), 3, "1 try + 2 retries");
}

#[tokio::test]
async fn unauthorized_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"errors":[{"code":32}]})))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(&server).home_timeline(false, 20, None).await.unwrap_err();
    assert_eq!(err, XError::Unauthorized { status: 401, code: Some(32) });
}

#[tokio::test]
async fn stale_query_id_is_rediscovered_and_retried_once() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH)).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    // Discovery: the page references a bundle declaring a new id.
    let js_url = format!("{}/x-web/x-web/entry-test.js", server.uri());
    Mock::given(method("GET"))
        .and(path("/home"))
        .and(header("cookie", format!("auth_token={TOKEN}; ct0={CSRF}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"<html><script src="{js_url}"></script></html>"#)),
        )
        .mount(&server)
        .await;
    Mock::given(path("/x-web/x-web/entry-test.js"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"a={queryId:"NEWID123",operationName:"HomeTimeline",operationType:"query",metadata:{featureSwitches:["f_one"],fieldToggles:[]}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(path("/i/api/graphql/NEWID123/HomeTimeline"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;

    let c = client_with_discovery(&server);
    assert_eq!(c.home_timeline(false, 20, None).await.unwrap(), ok_body());

    // The retry used the discovered feature switches, not the full default map.
    let reqs = server.received_requests().await.unwrap();
    let retry = reqs.iter().find(|r| r.url.path().contains("NEWID123")).unwrap();
    let q: std::collections::HashMap<_, _> = retry.url.query_pairs().into_owned().collect();
    let features: Value = serde_json::from_str(&q["features"]).unwrap();
    assert_eq!(features, json!({"f_one": true}));
}

#[tokio::test]
async fn stale_query_id_without_newer_id_reports_stale() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH)).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    Mock::given(path("/home"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<script src="{}/x-web/x-web/e.js"></script>"#,
            server.uri()
        )))
        .mount(&server)
        .await;
    Mock::given(path("/x-web/x-web/e.js"))
        .respond_with(ResponseTemplate::new(200).set_body_string("nothing here"))
        .mount(&server)
        .await;
    let err = client_with_discovery(&server).home_timeline(false, 20, None).await.unwrap_err();
    assert_eq!(
        err,
        XError::StaleQueryId {
            operation: "HomeTimeline".into(),
            query_id: "0toR0MxC06yY3Fc57GXBpA".into()
        }
    );
}

#[tokio::test]
async fn override_skips_discovery_on_404() {
    let server = MockServer::start().await;
    Mock::given(path("/i/api/graphql/MANUAL/HomeTimeline"))
        .respond_with(ResponseTemplate::new(404))
        .expect(2) // GET, then the POST fallback
        .mount(&server)
        .await;
    let c = client_with_discovery(&server);
    c.catalog().set_override("HomeTimeline", "MANUAL");
    assert!(matches!(
        c.home_timeline(false, 20, None).await,
        Err(XError::StaleQueryId { query_id, .. }) if query_id == "MANUAL"
    ));
    // No discovery page was requested.
    assert!(server.received_requests().await.unwrap().iter().all(|r| r.url.path() != "/home"));
}

#[tokio::test]
async fn mutations_post_json_with_query_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/i/api/graphql/lI07N6Otwv1PhnEgXILM7A/FavoriteTweet"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"favorite_tweet":"Done"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    client(&server).graphql("FavoriteTweet", json!({"tweet_id": "123"})).await.unwrap();
    let req: &Request = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["queryId"], "lI07N6Otwv1PhnEgXILM7A");
    assert_eq!(body["variables"]["tweet_id"], "123");
    assert!(body["features"].is_object());
    assert_eq!(req.headers["content-type"], "application/json");
}

#[tokio::test]
async fn rest_posts_send_a_form_with_the_session_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/i/api/1.1/friendships/create.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id_str": "42"})))
        .expect(1)
        .mount(&server)
        .await;
    let fields =
        vec![("user_id".to_owned(), "42".to_owned()), ("a b".to_owned(), "c&d".to_owned())];
    let body = client(&server).rest_post("1.1/friendships/create.json", fields).await.unwrap();
    assert_eq!(body, json!({"id_str": "42"}));
    let req: &Request = &server.received_requests().await.unwrap()[0];
    assert_eq!(req.headers["content-type"], "application/x-www-form-urlencoded");
    let sent: Vec<(String, String)> = url::form_urlencoded::parse(&req.body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(sent, [("user_id".into(), "42".into()), ("a b".into(), "c&d".into())]);
    assert_eq!(req.headers["x-csrf-token"], CSRF);
    assert!(req.headers["cookie"].to_str().unwrap().contains(&format!("ct0={CSRF}")));
    assert!(req.headers.contains_key("x-client-transaction-id"));
    assert!(req.headers["authorization"].to_str().unwrap().starts_with("Bearer "));
}

/// Matches when the `variables` query parameter is this JSON, in any key order.
struct Variables(Value);

impl wiremock::Match for Variables {
    fn matches(&self, request: &Request) -> bool {
        request
            .url
            .query_pairs()
            .find(|(k, _)| k == "variables")
            .and_then(|(_, v)| serde_json::from_str::<Value>(&v).ok())
            .is_some_and(|v| v == self.0)
    }
}

#[tokio::test]
async fn cursor_is_forwarded() {
    let server = MockServer::start().await;
    Mock::given(path("/i/api/graphql/KQPfcqQJJqF6M0D77p3JKw/HomeLatestTimeline"))
        .and(Variables(json!({
            "count": 20,
            "cursor": "DAABCgAB",
            "includePromotedContent": true,
            "latestControlAvailable": true,
            "seenTweetIds": [],
            "withCommunity": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;
    client(&server).home_timeline(true, 20, Some("DAABCgAB")).await.unwrap();
}

#[tokio::test]
async fn secrets_never_appear_in_debug_or_errors() {
    let server = MockServer::start().await;
    Mock::given(path(HOME_PATH)).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let c = client(&server);
    let err = c.home_timeline(false, 20, None).await.unwrap_err();
    let shown = format!("{c:?} {err} {err:?} {:?}", c.transport().session());
    assert!(!shown.contains(TOKEN), "{shown}");
    assert!(!shown.contains(CSRF), "{shown}");
}

#[tokio::test]
async fn unknown_operation_is_an_error() {
    let server = MockServer::start().await;
    let err = client(&server).graphql("NoSuchOperation", json!({})).await.unwrap_err();
    assert_eq!(err, XError::UnknownOperation("NoSuchOperation".into()));
}

#[tokio::test]
async fn discovery_follows_inline_webpack_chunk_map_and_stops_early() {
    let server = MockServer::start().await;
    let base = format!("{}/responsive-web/client-web", server.uri());
    // One relevant chunk and 11 unrelated ones (more than one download batch).
    let unrelated = 11;
    let names: String =
        (0..unrelated).map(|i| format!(r#",{}:"bundle.Unrelated{i}""#, i + 2)).collect();
    let hashes: String =
        (0..unrelated).map(|i| format!(r#",{}:"ffeeddccbbaa99{i:02}""#, i + 2)).collect();
    Mock::given(path("/home"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<script src="{base}/main.abc.js"></script>
               <script>m={{1:"loader.UserHandler"{names}}}[e]+"."+{{1:"00112233445566aa"{hashes}}}[e]+"a.js"</script>"#
        )))
        .mount(&server)
        .await;
    Mock::given(path("/responsive-web/client-web/main.abc.js"))
        .respond_with(ResponseTemplate::new(200).set_body_string("no operations here"))
        .mount(&server)
        .await;
    Mock::given(path("/responsive-web/client-web/loader.UserHandler.00112233445566aaa.js"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"e.exports={queryId:"KQPfcqQJJqF6M0D77p3JKw",operationName:"HomeLatestTimeline",operationType:"query",metadata:{featureSwitches:[],fieldToggles:[]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path_regex(r"bundle\.Unrelated"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&server)
        .await;

    let page = Url::parse(&format!("{}/home", server.uri())).unwrap();
    let targets = ["HomeLatestTimeline".to_owned()].into_iter().collect();
    let (ops, report) = Discovery::new(vec![page]).unwrap().run(None, &targets).await.unwrap();
    assert_eq!(ops["HomeLatestTimeline"].query_id, "KQPfcqQJJqF6M0D77p3JKw");
    // main.js + the relevant chunk come first; the first batch of 8 found the
    // target, so the remaining unrelated chunks were never downloaded.
    let fetched = report.scripts_by_family["responsive-web"];
    assert_eq!(fetched, 8, "one batch");
    assert!(fetched < 2 + unrelated);
}

#[tokio::test]
async fn query_served_only_over_post_falls_back_and_is_remembered() {
    let server = MockServer::start().await;
    let search = "/i/api/graphql/f7SuGIm11eLbQs7c0K9X9w/SearchTimeline";
    Mock::given(method("GET"))
        .and(path(search))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(search))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(2)
        .mount(&server)
        .await;
    let c = client(&server);
    c.graphql("SearchTimeline", json!({"rawQuery": "rust"})).await.unwrap();
    // Second call goes straight to POST (the GET mock expects exactly one hit).
    c.graphql("SearchTimeline", json!({"rawQuery": "rust"})).await.unwrap();
    let post = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let body: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body["variables"]["rawQuery"], "rust");
    assert_eq!(body["queryId"], "f7SuGIm11eLbQs7c0K9X9w");
}
