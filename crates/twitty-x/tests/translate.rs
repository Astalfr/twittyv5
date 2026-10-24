//! Translation of real (anonymised) Twitter responses into `twitty-model`.
//!
//! Fixtures in `tests/fixtures/` are live captures scrubbed with
//! `twitty-x-cli anonymise` (see docs/x-api.md). Snapshots record a compact,
//! readable summary of the *model* output: when Twitter changes its JSON, the fix
//! goes into `translate` and these snapshots must not change.

#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] fns")]

use std::fmt::Write as _;

use twitty_model::{Embed, FeedItem, FeedPage, FollowAction, Media, Post, PostOrGone, SpanKind};
use twitty_x::translate::{Translated, feed_page, follow_status, users};

const FIXTURES: &[&str] = &[
    "home",
    "home--latest",
    "usertweets-elonmusk",
    "usertweets-nasa",
    "search-quotes",
    "search-videos",
    "search-rust",
    "tweetdetail",
    "notifications",
    "userreplies-nasa",
    "usermedia-nasa",
    "usermedia-nasa-2",
    "list-space",
    "search-translated",
    "tweetdetail-note",
];

fn load(name: &str) -> Translated {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    feed_page(&serde_json::from_str(&raw).unwrap())
}

fn short(s: &str, n: usize) -> String {
    let flat = s.replace('\n', "⏎");
    let mut out: String = flat.chars().take(n).collect();
    if flat.chars().count() > n {
        out.push('…');
    }
    out
}

fn post_line(p: &Post, indent: &str) -> String {
    let mut s = format!(
        "{indent}post {} @{} {} \"{}\"",
        p.id,
        p.author.handle,
        p.created_at.format("%Y-%m-%dT%H:%M:%SZ"),
        short(p.body.text(), 50)
    );
    for span in p.body.spans() {
        let kind = match &span.kind {
            SpanKind::Mention(_) => "mention",
            SpanKind::Hashtag { .. } => "hashtag",
            SpanKind::Cashtag { .. } => "cashtag",
            SpanKind::Link { .. } => "link",
        };
        let _ = write!(s, " [{kind} {}]", short(&p.body.text()[span.range.clone()], 20));
    }
    for m in &p.media {
        let (kind, extra) = match m {
            Media::Image { .. } => ("image", String::new()),
            Media::Video { variants, duration, .. } => (
                "video",
                format!(
                    " {}s{}",
                    duration.map_or(0, |d| d.as_secs()),
                    if variants.is_empty() {
                        String::new()
                    } else {
                        format!(" mp4 ×{}", variants.len())
                    }
                ),
            ),
            Media::Gif { stream, .. } => {
                ("gif", if stream.is_some() { " mp4".into() } else { String::new() })
            }
        };
        let size = m.preview().size.map(|(w, h)| format!(" {w}x{h}")).unwrap_or_default();
        let _ = write!(s, " {{{kind}{size}{extra}}}");
    }
    if let Some(r) = &p.reply_to {
        let _ = write!(s, " ↩{}", r.handle.as_deref().unwrap_or("?"));
    }
    let m = &p.metrics;
    let _ =
        write!(s, " ♥{} ⟳{} 💬{} ❝{} 🔖{}", m.likes, m.reposts, m.replies, m.quotes, m.bookmarks);
    if let Some(v) = m.views {
        let _ = write!(s, " 👁{v}");
    }
    if let Some(t) = &p.translation {
        let _ = write!(
            s,
            "
{indent}  translated {}→{}: \"{}\"",
            t.from.as_deref().unwrap_or("?"),
            t.to.as_deref().unwrap_or("?"),
            short(t.body.text(), 50)
        );
        for span in t.body.spans() {
            let _ = write!(s, " [{}]", short(&t.body.text()[span.range.clone()], 20));
        }
    }
    if let Some(n) = &p.community_note {
        let _ = write!(
            s,
            "
{indent}  note: \"{}\" → {}",
            short(n.body.text(), 50),
            n.url
        );
        for span in n.body.spans() {
            let _ = write!(s, " [{}]", short(&n.body.text()[span.range.clone()], 20));
        }
    }
    match &p.embed {
        Some(Embed::Quote { quoted }) => match quoted.as_ref() {
            PostOrGone::Post(q) => {
                let _ = write!(s, "\n{indent}  quote:\n{}", post_line(q, &format!("{indent}    ")));
            }
            PostOrGone::Unavailable { reason } => {
                let _ = write!(s, "\n{indent}  quote: unavailable {reason:?}");
            }
        },
        Some(Embed::Link(l)) => {
            let _ = write!(
                s,
                "\n{indent}  card: \"{}\" ({}){}",
                short(&l.title, 30),
                short(&l.domain, 30),
                if l.image.is_some() { " +image" } else { "" }
            );
        }
        Some(Embed::Poll(poll)) => {
            let _ = write!(
                s,
                "\n{indent}  poll: {} options, {} votes",
                poll.options.len(),
                poll.total_votes()
            );
        }
        None => {}
    }
    s
}

fn summary(page: &FeedPage) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "newer cursor: {}", page.newer.is_some());
    let _ = writeln!(out, "older cursor: {}", page.older.is_some());
    for item in &page.items {
        let line = match item {
            FeedItem::Post(p) => post_line(p, ""),
            FeedItem::Repost { by, post } => {
                format!("repost by @{}\n{}", by.handle, post_line(post, "  "))
            }
            FeedItem::Thread(posts) => {
                let mut s = format!("thread of {}", posts.len());
                for p in posts {
                    s.push('\n');
                    s.push_str(&post_line(p, "  "));
                }
                s
            }
            FeedItem::Notification(n) => {
                let actors: Vec<&str> = n.actors.iter().map(|a| a.handle.as_str()).collect();
                let mut s = format!(
                    "notification {:?} {} by {:?} \"{}\"",
                    n.kind,
                    n.created_at.format("%Y-%m-%dT%H:%M:%SZ"),
                    actors,
                    short(n.text.text(), 40)
                );
                if let Some(t) = &n.target {
                    s.push('\n');
                    s.push_str(&post_line(t, "  "));
                }
                s
            }
            FeedItem::Unavailable { entry, reason, .. } => {
                format!("unavailable {entry} {reason:?}")
            }
            FeedItem::Unsupported { entry, reason } => format!("UNSUPPORTED {entry}: {reason}"),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[test]
fn fixtures_translate_without_unsupported_rows_or_warnings() {
    for name in FIXTURES {
        let t = load(name);
        let unsupported: Vec<_> =
            t.page.items.iter().filter(|i| matches!(i, FeedItem::Unsupported { .. })).collect();
        assert!(unsupported.is_empty(), "{name}: {unsupported:#?}");
        assert!(t.warnings.is_empty(), "{name}: {:#?}", t.warnings);
    }
}

#[test]
fn fixture_snapshots() {
    for name in FIXTURES {
        insta::assert_snapshot!(*name, summary(&load(name).page));
    }
}

#[test]
fn fixtures_cover_every_shape_we_translate() {
    let pages: Vec<FeedPage> = FIXTURES.iter().map(|n| load(n).page).collect();
    let items: Vec<&FeedItem> = pages.iter().flat_map(|p| &p.items).collect();
    let posts: Vec<&Post> = items.iter().flat_map(|i| i.posts()).collect();
    let has = |f: &dyn Fn(&&FeedItem) -> bool| items.iter().any(f);
    let has_post = |f: &dyn Fn(&&Post) -> bool| posts.iter().any(f);

    assert!(has(&|i| matches!(i, FeedItem::Repost { .. })), "repost");
    assert!(has(&|i| matches!(i, FeedItem::Thread(_))), "thread");
    assert!(has(&|i| matches!(i, FeedItem::Notification(_))), "notification");
    assert!(has_post(&|p| matches!(&p.embed, Some(Embed::Quote { .. }))), "quote");
    assert!(has_post(&|p| matches!(&p.embed, Some(Embed::Link(_)))), "link card");
    assert!(
        has_post(&|p| p
            .media
            .iter()
            .any(|m| matches!(m, Media::Video { .. }) && m.stream().is_some())),
        "video"
    );
    assert!(has_post(&|p| p.media.iter().any(|m| matches!(m, Media::Gif { .. }))), "gif");
    assert!(has_post(&|p| p.media.len() >= 2), "multi-image");
    assert!(has_post(&|p| p.reply_to.is_some()), "reply");
    assert!(has_post(&|p| p.body.text().chars().count() > 280), "long note post");
    assert!(has_post(&|p| p.translation.is_some()), "translated post");
    assert!(
        has_post(&|p| p.community_note.as_ref().is_some_and(|n| !n.body.spans().is_empty())),
        "community note with a link"
    );
    for kind in ["mention", "hashtag", "link"] {
        let found = has_post(&|p| {
            p.body.spans().iter().any(|s| {
                matches!(
                    (&s.kind, kind),
                    (SpanKind::Mention(_), "mention")
                        | (SpanKind::Hashtag { .. }, "hashtag")
                        | (SpanKind::Link { .. }, "link")
                )
            })
        });
        assert!(found, "{kind} span");
    }
    assert!(pages.iter().all(|p| p.items.is_empty() || p.older.is_some()), "bottom cursors");
}

#[test]
fn media_grid_pages_carry_posts() {
    // Page 1 is a grid module; page 2 only adds to it (`TimelineAddToModule`).
    for name in ["usermedia-nasa", "usermedia-nasa-2"] {
        let page = load(name).page;
        let posts: Vec<_> = page.items.iter().flat_map(FeedItem::posts).collect();
        assert!(posts.len() >= 5, "{name}: {}", posts.len());
        assert!(posts.iter().all(|p| !p.media.is_empty()), "{name}: every grid post has media");
        assert!(page.older.is_some(), "{name}");
    }
}

#[test]
fn promoted_posts_and_prompts_are_not_shown() {
    let home = load("home");
    // The capture had 28 organic tweets and 7 promoted ones.
    assert_eq!(home.page.items.len(), 28);
    // The "Following" capture only had a cover and a Grok prompt.
    assert!(load("home--latest").page.items.is_empty());
}

#[test]
fn authors_are_shared_within_a_page() {
    let page = load("usertweets-nasa").page;
    let authors: Vec<_> = page.items.iter().flat_map(|i| i.posts()).map(|p| &p.author).collect();
    let first = authors[0];
    let same: Vec<_> = authors.iter().filter(|a| a.id == first.id).collect();
    assert!(same.len() > 1);
    assert!(same.iter().all(|a| std::sync::Arc::ptr_eq(a, first)), "one Arc per author");
}

/// The v1.1 `friendships/create` replies (hand-written, no real account):
/// what each follow change reads as.
#[test]
fn follow_replies() {
    let mut out = String::new();
    for name in ["friendships-create", "friendships-create-protected"] {
        let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let body = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for action in [FollowAction::Follow, FollowAction::Unfollow] {
            let _ = writeln!(out, "{name} {action:?} → {:?}", follow_status(&body, action));
        }
    }
    insta::assert_snapshot!("follow-replies", out);
}

/// The "People" tab of `SearchTimeline`: accounts, not tweets.
#[test]
fn people_search_lists_accounts() {
    let path = format!("{}/tests/fixtures/search-people.json", env!("CARGO_MANIFEST_DIR"));
    let body = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let t = users(&body);
    assert!(t.warnings.is_empty(), "{:#?}", t.warnings);
    assert_eq!(t.users.len(), 20);
    assert!(feed_page(&body).page.items.is_empty(), "accounts are not feed rows");
    let mut out = String::new();
    for p in &t.users {
        let _ = writeln!(
            out,
            "{} @{} \"{}\" {:?} followers={} avatar={}",
            p.id,
            p.handle,
            short(&p.display_name, 30),
            p.badges.verified,
            p.counts.followers,
            p.avatar.is_some()
        );
    }
    insta::assert_snapshot!("search-people", out);
}
