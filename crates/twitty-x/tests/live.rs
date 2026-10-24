//! Opt-in checks against real Twitter data. Never run in CI.

#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test helpers outside #[test] fns")]
//!
//! - `TWITTY_BUNDLE_DIR=<dir> cargo test -p twitty-x --test live -- --ignored bundles`
//!   runs the extractors over downloaded x.com JavaScript (not committed).
//! - `TWITTY_LIVE=1 TWITTY_AUTH_TOKEN=… TWITTY_CT0=… cargo test -p twitty-x --test live -- --ignored home`
//!   calls the real API with a (secondary) account.

use std::sync::Arc;

use twitty_x::{
    Session, XClient,
    catalog::Catalog,
    discovery::{Discovery, Format, extract_operations},
    transport::{ClientIds, RetryPolicy, Transport},
};
use url::Url;

#[test]
#[ignore = "needs TWITTY_BUNDLE_DIR with downloaded x.com bundles"]
fn bundles_extract_operations() {
    let dir = std::env::var("TWITTY_BUNDLE_DIR").expect("set TWITTY_BUNDLE_DIR");
    let mut legacy = 0;
    let mut relay = 0;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "js") {
            for op in extract_operations(&std::fs::read_to_string(&path).unwrap()) {
                match op.format {
                    Format::Legacy => legacy += 1,
                    Format::Relay => relay += 1,
                }
                names.push(op.name);
            }
        }
    }
    names.sort();
    names.dedup();
    println!("legacy: {legacy}, relay: {relay}, distinct names: {}", names.len());
    println!(
        "timeline-like: {:?}",
        names.iter().filter(|n| n.contains("Timeline")).collect::<Vec<_>>()
    );
    assert!(legacy + relay > 0, "no operations found: the bundle format changed");
}

fn live_client() -> XClient {
    assert_eq!(std::env::var("TWITTY_LIVE").as_deref(), Ok("1"), "set TWITTY_LIVE=1");
    let session = Session::from_env().expect("TWITTY_AUTH_TOKEN / TWITTY_CT0");
    let transport = Transport::new(
        session,
        &ClientIds::random(),
        Url::parse(Transport::API_BASE).unwrap(),
        RetryPolicy::default(),
    )
    .unwrap();
    XClient::new(transport, Arc::new(Catalog::new(None)), Some(Discovery::default_pages().unwrap()))
}

#[tokio::test]
#[ignore = "calls the real X API; needs TWITTY_LIVE=1 and a session"]
async fn home_timeline_returns_instructions() {
    let body = live_client().home_timeline(false, 20, None).await.unwrap();
    assert!(
        body.pointer("/data/home/home_timeline_urt/instructions").is_some_and(|i| i.is_array()),
        "unexpected shape: top-level keys {:?}",
        body.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );
}

#[tokio::test]
#[ignore = "calls the real X API; needs TWITTY_LIVE=1 and a session"]
async fn people_search_finds_accounts() {
    let backend = twitty_x::XBackend::new(Arc::new(live_client()));
    let users = twitty_model::Backend::search_users(&backend, "nasa").await.unwrap();
    assert!(!users.is_empty(), "no account found for \"nasa\"");
    let unreadable: u64 =
        twitty_model::Backend::health(&backend).iter().map(|h| h.unsupported + h.warnings).sum();
    assert_eq!(unreadable, 0, "{:#?}", twitty_model::Backend::health(&backend));
}

#[tokio::test]
#[ignore = "calls the real X API; needs TWITTY_LIVE=1 and a session"]
async fn viewer_lists_are_readable() {
    let backend = twitty_x::XBackend::new(Arc::new(live_client()));
    // The account may have no lists; what matters is that the page still parses.
    twitty_model::Backend::viewer_lists(&backend).await.unwrap();
    let health = twitty_model::Backend::health(&backend);
    let page = health.iter().find(|h| h.name == "ListsManagementPageTimeline").unwrap();
    assert_eq!((page.ok, page.unsupported, page.warnings), (1, 0, 0), "{page:#?}");
}

#[tokio::test]
#[ignore = "downloads x.com's public bundles; needs TWITTY_LIVE=1"]
async fn discovery_logged_out_finds_operations() {
    assert_eq!(std::env::var("TWITTY_LIVE").as_deref(), Ok("1"), "set TWITTY_LIVE=1");
    let page = Url::parse("https://x.com/?lang=en").unwrap();
    let (ops, report) =
        Discovery::new(vec![page]).unwrap().run(None, &Default::default()).await.unwrap();
    println!("{report:?}");
    println!("found {} operations", ops.len());
    assert!(!ops.is_empty(), "discovery found nothing on the logged-out site");
}
