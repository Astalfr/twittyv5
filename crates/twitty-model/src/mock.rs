//! A deterministic in-memory [`Backend`] for UI development and tests.
//!
//! Every feed is a fixed list of generated posts, served newest first in
//! pages of [`MockBackend::PAGE_SIZE`]. Tests can push new posts to the top,
//! inject failures, and observe actions.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::{DateTime, Duration, Utc};
use url::Url;

use crate::{
    Backend, BackendError, Badges, BoxFuture, Draft, Embed, FeedItem, FeedPage, FeedSource,
    FollowAction, FollowStatus, ImageRef, LinkCard, Media, Metrics, PageRequest, PageToken, Poll,
    PollOption, Post, PostAction, PostId, PostOrGone, Profile, ProfileCounts, RichText, Span,
    SpanKind, ThreadView, Translation, UnavailableReason, UserId, UserRef, UserTab, VerifiedKind,
    ViewerPostState,
};
use crate::{Notification, NotificationId, NotificationKind};

/// In-memory backend. Cheap to clone; clones share state.
#[derive(Debug, Clone)]
pub struct MockBackend {
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    /// Timestamp of the newest generated post.
    base: DateTime<Utc>,
    authors: Vec<Arc<Profile>>,
    /// Feed contents, newest first.
    feeds: HashMap<FeedSource, Vec<FeedItem>>,
    next_id: u64,
    failures: VecDeque<BackendError>,
    actions: Vec<PostAction>,
    viewer: HashMap<PostId, ViewerPostState>,
    follows: Vec<(UserId, FollowAction)>,
    /// Accounts the viewer followed or asked to follow.
    following: HashMap<UserId, FollowStatus>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    /// Rows per page.
    pub const PAGE_SIZE: usize = 20;
    /// Rows generated for each feed.
    pub const FEED_LEN: usize = 120;

    /// A backend with generated sample data.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                authors: sample_authors(),
                feeds: HashMap::new(),
                next_id: 1,
                failures: VecDeque::new(),
                actions: Vec::new(),
                viewer: HashMap::new(),
                follows: Vec::new(),
                following: HashMap::new(),
                base: base_time(),
            })),
        }
    }

    /// Makes the next call fail with `error`. Calls queue up in order.
    pub fn fail_next(&self, error: BackendError) {
        self.lock().failures.push_back(error);
    }

    /// Adds `count` new posts to the top of `source`, returning their ids.
    pub fn push_new(&self, source: &FeedSource, count: usize) -> Vec<PostId> {
        let mut st = self.lock();
        st.ensure_feed(source);
        let newest = st.newest_time(source);
        let mut ids = Vec::with_capacity(count);
        let mut fresh = Vec::with_capacity(count);
        for i in 0..count {
            let post = st.make_post(newest + Duration::minutes(i as i64 + 1));
            ids.push(post.id.clone());
            fresh.push(FeedItem::Post(post));
        }
        fresh.reverse();
        if let Some(feed) = st.feeds.get_mut(source) {
            feed.splice(0..0, fresh);
        }
        ids
    }

    /// Actions received so far.
    pub fn actions(&self) -> Vec<PostAction> {
        self.lock().actions.clone()
    }

    /// Follow changes received so far.
    pub fn follows(&self) -> Vec<(UserId, FollowAction)> {
        self.lock().follows.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means a test panicked while holding it; the data is still usable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn take_failure(&self) -> Result<(), BackendError> {
        match self.lock().failures.pop_front() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn page(&self, source: &FeedSource, page: PageRequest) -> Result<FeedPage, BackendError> {
        self.take_failure()?;
        let mut st = self.lock();
        st.ensure_feed(source);
        let viewer = st.viewer.clone();
        let feed = st.feeds.get(source).map(Vec::as_slice).unwrap_or_default();
        let pos = |key: &str| feed.iter().position(|item| item.key() == key);

        let range = match &page {
            PageRequest::First => 0..feed.len().min(Self::PAGE_SIZE),
            PageRequest::Older(token) => {
                let start = decode(token).and_then(pos).map_or(feed.len(), |i| i + 1);
                start..(start + Self::PAGE_SIZE).min(feed.len())
            }
            PageRequest::Newer(token) => {
                let end = decode(token).and_then(pos).unwrap_or(0);
                0..end
            }
        };
        let items: Vec<FeedItem> =
            feed[range.clone()].iter().cloned().map(|i| apply_viewer(i, &viewer)).collect();

        // Newer cursor points at the newest item we know of; older at the last served.
        let newer = match (&page, items.first()) {
            (PageRequest::Newer(t), None) => Some(t.clone()),
            (_, Some(first)) => Some(encode(&first.key())),
            (_, None) => feed.first().map(|f| encode(&f.key())),
        };
        let older = match &page {
            PageRequest::Newer(_) => None,
            _ if range.end < feed.len() => items.last().map(|l| encode(&l.key())),
            _ => None,
        };
        Ok(FeedPage { items, newer, older })
    }

    fn find_post(&self, id: &PostId) -> Option<Post> {
        let st = self.lock();
        st.feeds.values().flatten().flat_map(|item| item.posts()).find(|p| &p.id == id).cloned()
    }
}

fn encode(key: &str) -> PageToken {
    PageToken::new(format!("mock:{key}"))
}

fn decode(token: &PageToken) -> Option<&str> {
    token.as_str().strip_prefix("mock:")
}

fn apply_viewer(item: FeedItem, viewer: &HashMap<PostId, ViewerPostState>) -> FeedItem {
    let fix = |mut p: Post| {
        if let Some(v) = viewer.get(&p.id) {
            p.viewer = *v;
        }
        p
    };
    match item {
        FeedItem::Post(p) => FeedItem::Post(fix(p)),
        FeedItem::Repost { by, post } => FeedItem::Repost { by, post: fix(post) },
        FeedItem::Thread(ps) => FeedItem::Thread(ps.into_iter().map(fix).collect()),
        other => other,
    }
}

impl State {
    fn ensure_feed(&mut self, source: &FeedSource) {
        if self.feeds.contains_key(source) {
            return;
        }
        let start = self.base;
        let notifications = matches!(source, FeedSource::Notifications { .. });
        let items: Vec<FeedItem> = (0..MockBackend::FEED_LEN)
            .map(|i| {
                let at = start - Duration::minutes(i as i64 * 7);
                if notifications { self.make_notification(i, at) } else { self.make_item(i, at) }
            })
            .collect();
        let items = match source {
            FeedSource::User { id, tab, .. } => self.user_feed(id, *tab, items),
            _ => items,
        };
        self.feeds.insert(source.clone(), items);
    }

    /// A known author's posts are theirs; their media tab only has posts
    /// with media.
    fn user_feed(&self, id: &UserId, tab: UserTab, items: Vec<FeedItem>) -> Vec<FeedItem> {
        let Some(user) = self.authors.iter().find(|a| &a.id == id) else {
            return items;
        };
        items
            .into_iter()
            .filter_map(|item| match item {
                FeedItem::Post(mut p) => {
                    p.author = Arc::clone(user);
                    (tab != UserTab::Media || !p.media.is_empty()).then_some(FeedItem::Post(p))
                }
                other => (tab != UserTab::Media).then_some(other),
            })
            .collect()
    }

    fn newest_time(&self, source: &FeedSource) -> DateTime<Utc> {
        self.feeds
            .get(source)
            .and_then(|f| f.iter().flat_map(|i| i.posts()).map(|p| p.created_at).max())
            .unwrap_or(self.base)
    }

    fn make_item(&mut self, i: usize, at: DateTime<Utc>) -> FeedItem {
        match i % 17 {
            5 => {
                let by = self.author(i + 3);
                FeedItem::Repost { by, post: self.make_post(at) }
            }
            11 => {
                let a = self.make_post(at - Duration::minutes(3));
                let mut b = self.make_post(at);
                b.reply_to = Some(crate::ReplyRef {
                    post: Some(a.id.clone()),
                    user: Some(a.author.id.clone()),
                    handle: Some(a.author.handle.clone()),
                });
                FeedItem::Thread(vec![a, b])
            }
            16 => {
                // Tombstones frequently come without a post id.
                let entry = format!("tombstone-{}", self.next_post_id());
                FeedItem::Unavailable { entry, id: None, reason: UnavailableReason::Deleted }
            }
            _ => FeedItem::Post(self.make_post(at)),
        }
    }

    fn make_notification(&mut self, i: usize, at: DateTime<Utc>) -> FeedItem {
        let kind = match i % 5 {
            0 => NotificationKind::Like,
            1 => NotificationKind::Repost,
            2 => NotificationKind::Follow,
            3 => NotificationKind::Quote,
            // Replies and mentions arrive as plain posts.
            _ => return FeedItem::Post(self.make_post(at)),
        };
        let actors: Vec<Arc<Profile>> = (0..1 + i % 3).map(|k| self.author(i + k)).collect();
        let first = &actors[0].display_name;
        let who = match actors.len() {
            1 => first.clone(),
            2 => format!("{first} and 1 other"),
            n => format!("{first} and {} others", n - 1),
        };
        let what = match kind {
            NotificationKind::Like => "liked your post",
            NotificationKind::Repost => "reposted your post",
            NotificationKind::Follow => "followed you",
            NotificationKind::Quote => "quoted your post",
            NotificationKind::Other => "did something",
        };
        let target = (kind != NotificationKind::Follow).then(|| {
            let mut post = self.make_post(at - Duration::hours(1));
            post.author = Arc::clone(&self.authors[0]);
            Box::new(post)
        });
        let id = NotificationId::new(format!("n{}", self.next_post_id()));
        FeedItem::Notification(Notification {
            id,
            kind,
            created_at: at,
            text: RichText::plain(format!("{who} {what}")),
            actors,
            target,
        })
    }

    fn next_post_id(&mut self) -> PostId {
        let id = PostId::new(format!("{}", 1_900_000_000_000_000_000u64 + self.next_id));
        self.next_id += 1;
        id
    }

    fn author(&self, n: usize) -> Arc<Profile> {
        Arc::clone(&self.authors[n % self.authors.len()])
    }

    fn make_post(&mut self, at: DateTime<Utc>) -> Post {
        let id = self.next_post_id();
        let n = self.next_id as usize;
        let author = self.author(n);
        let (body, lang, translation) = if n % 10 == 5 {
            // A post in another language, with the translation a provider sends.
            let translated = Translation {
                body: RichText::plain(
                    "Native rendering makes scrolling smooth, even with thousands of posts.",
                ),
                from: Some("fr".into()),
                to: Some("en".into()),
            };
            let original =
                "Le rendu natif rend le défilement fluide, même avec des milliers de posts.";
            (RichText::plain(original), Some("fr".into()), Some(translated))
        } else {
            (sample_body(n, &self.authors), Some("en".into()), None)
        };
        let media = match n % 9 {
            1 => vec![photo(n, 0, 1200, 800)],
            3 => vec![photo(n, 0, 800, 800), photo(n, 1, 800, 800)],
            6 => (0..4).map(|k| photo(n, k, 900, 600)).collect(),
            7 => vec![Media::Video {
                poster: image(&format!("https://picsum.photos/seed/v{n}/1280/720"), 1280, 720),
                variants: Vec::new(),
                duration: Some(std::time::Duration::from_secs(42)),
                alt: None,
            }],
            _ => Vec::new(),
        };
        let embed = match n % 13 {
            4 => Some(Embed::Link(LinkCard {
                url: url("https://www.rust-lang.org/"),
                title: "Rust Programming Language".into(),
                description: Some(
                    "A language empowering everyone to build reliable software.".into(),
                ),
                image: Some(image(&format!("https://picsum.photos/seed/c{n}/1200/628"), 1200, 628)),
                domain: "rust-lang.org".into(),
            })),
            8 => Some(Embed::Poll(Poll {
                options: vec![
                    PollOption { label: "GPUI".into(), votes: 120 + n as u64 },
                    PollOption { label: "egui".into(), votes: 80 },
                    PollOption { label: "Slint".into(), votes: 42 },
                ],
                ends_at: Some(at + Duration::days(1)),
                closed: false,
            })),
            10 => {
                let quoted_author = self.author(n + 1);
                let quoted_id = self.next_post_id();
                Some(Embed::Quote {
                    quoted: Box::new(PostOrGone::Post(Post {
                        id: quoted_id.clone(),
                        url: post_url(&quoted_author.handle, &quoted_id),
                        author: quoted_author,
                        created_at: at - Duration::hours(2),
                        body: RichText::plain("The quoted post, which has its own text."),
                        media: Vec::new(),
                        embed: None,
                        reply_to: None,
                        metrics: Metrics::default(),
                        viewer: ViewerPostState::default(),
                        lang: Some("en".into()),
                        translation: None,
                    })),
                })
            }
            12 => Some(Embed::Quote {
                quoted: Box::new(PostOrGone::Unavailable { reason: UnavailableReason::Deleted }),
            }),
            _ => None,
        };
        Post {
            url: post_url(&author.handle, &id),
            id,
            author,
            created_at: at,
            body,
            media,
            embed,
            reply_to: None,
            metrics: Metrics {
                replies: (n * 3 % 40) as u64,
                reposts: (n * 7 % 300) as u64,
                quotes: (n % 12) as u64,
                likes: (n * 37 % 5000) as u64,
                bookmarks: (n % 30) as u64,
                views: (!n.is_multiple_of(4)).then_some((n * 1234 % 900_000) as u64),
            },
            viewer: ViewerPostState::default(),
            lang,
            translation,
        }
    }
}

impl Backend for MockBackend {
    fn viewer(&self) -> BoxFuture<'_, Result<Profile, BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            Ok(self.lock().authors[0].as_ref().clone())
        })
    }

    fn feed<'a>(
        &'a self,
        source: &'a FeedSource,
        page: PageRequest,
    ) -> BoxFuture<'a, Result<FeedPage, BackendError>> {
        Box::pin(async move { self.page(source, page) })
    }

    fn thread<'a>(&'a self, id: &'a PostId) -> BoxFuture<'a, Result<ThreadView, BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            let focus = self.find_post(id).ok_or(BackendError::NotFound)?;
            let replies = {
                let mut st = self.lock();
                (0..3)
                    .map(|k| {
                        let mut p = st.make_post(focus.created_at + Duration::minutes(k + 1));
                        p.reply_to = Some(crate::ReplyRef {
                            post: Some(focus.id.clone()),
                            user: Some(focus.author.id.clone()),
                            handle: Some(focus.author.handle.clone()),
                        });
                        FeedItem::Post(p)
                    })
                    .collect()
            };
            Ok(ThreadView { ancestors: Vec::new(), focus, replies, more_replies: None })
        })
    }

    fn profile<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, Result<Profile, BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            let st = self.lock();
            let mut p = st
                .authors
                .iter()
                .find(|a| a.handle.eq_ignore_ascii_case(handle))
                .map(|a| a.as_ref().clone())
                .ok_or(BackendError::NotFound)?;
            if let Some(status) = st.following.get(&p.id) {
                p.viewer.following = *status == FollowStatus::Following;
            }
            Ok(p)
        })
    }

    fn act(&self, action: PostAction) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            let mut st = self.lock();
            let v = st.viewer.entry(action.post_id().clone()).or_default();
            match &action {
                PostAction::Like(_) => v.liked = true,
                PostAction::Unlike(_) => v.liked = false,
                PostAction::Repost(_) => v.reposted = true,
                PostAction::Unrepost(_) => v.reposted = false,
                PostAction::Bookmark(_) => v.bookmarked = true,
                PostAction::Unbookmark(_) => v.bookmarked = false,
            }
            st.actions.push(action);
            Ok(())
        })
    }

    fn set_following<'a>(
        &'a self,
        user: &'a UserId,
        action: FollowAction,
    ) -> BoxFuture<'a, Result<FollowStatus, BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            let mut st = self.lock();
            let protected = st.authors.iter().any(|a| &a.id == user && a.badges.protected);
            let status = match action {
                FollowAction::Unfollow => FollowStatus::NotFollowing,
                FollowAction::Follow if protected => FollowStatus::Requested,
                FollowAction::Follow => FollowStatus::Following,
            };
            st.following.insert(user.clone(), status);
            st.follows.push((user.clone(), action));
            Ok(status)
        })
    }

    fn publish(&self, draft: Draft) -> BoxFuture<'_, Result<Post, BackendError>> {
        Box::pin(async move {
            self.take_failure()?;
            let mut st = self.lock();
            let id = st.next_post_id();
            let author = Arc::clone(&st.authors[0]);
            Ok(Post {
                url: post_url(&author.handle, &id),
                id,
                author,
                created_at: Utc::now(),
                body: RichText::plain(draft.text),
                media: Vec::new(),
                embed: None,
                reply_to: draft.reply_to.map(|p| crate::ReplyRef {
                    post: Some(p),
                    user: None,
                    handle: None,
                }),
                metrics: Metrics::default(),
                viewer: ViewerPostState::default(),
                lang: None,
                translation: None,
            })
        })
    }
}

/// "Now", truncated to the minute so generated times look natural.
fn base_time() -> DateTime<Utc> {
    let now = Utc::now();
    now - Duration::seconds(now.timestamp() % 60)
}

#[expect(clippy::expect_used, reason = "only called with URLs built from literals in this file")]
fn url(s: &str) -> Url {
    Url::parse(s).expect("mock URLs are valid")
}

fn image(s: &str, w: u32, h: u32) -> ImageRef {
    ImageRef { url: url(s), size: Some((w, h)) }
}

fn photo(n: usize, k: usize, w: u32, h: u32) -> Media {
    Media::Image {
        image: image(&format!("https://picsum.photos/seed/p{n}x{k}/{w}/{h}"), w, h),
        alt: Some(format!("Sample photo {k}")),
    }
}

fn post_url(handle: &str, id: &PostId) -> Url {
    url(&format!("https://x.com/{handle}/status/{id}"))
}

fn sample_authors() -> Vec<Arc<Profile>> {
    let people = [
        ("ferris", "Ferris the Crab", Some(VerifiedKind::Individual)),
        ("zed", "Zed Industries", Some(VerifiedKind::Business)),
        ("ada", "Ada Lovelace", None),
        ("grace", "Grace Hopper", Some(VerifiedKind::Individual)),
        ("linus", "Linus", None),
        ("nasa", "NASA", Some(VerifiedKind::Government)),
        ("emoji_fan", "Emoji 🦀 Fan", None),
        ("long_name_account", "An Account With A Really Quite Long Display Name", None),
    ];
    people
        .iter()
        .enumerate()
        .map(|(i, (handle, name, verified))| {
            let mut p = Profile::new(UserId::new(format!("{}", 1000 + i)), *handle, *name);
            p.avatar = Some(image(&format!("https://i.pravatar.cc/96?u={handle}"), 96, 96));
            p.badges = Badges { verified: *verified, protected: i == 4 };
            p.counts = ProfileCounts {
                followers: 1000 * (i as u64 + 1),
                following: 100 + i as u64,
                posts: 5000 + i as u64,
            };
            p.bio = RichText::plain(format!("Sample bio for @{handle}."));
            Arc::new(p)
        })
        .collect()
}

fn sample_body(n: usize, authors: &[Arc<Profile>]) -> RichText {
    const PLAIN: [&str; 6] = [
        "Shipping a native client feels so good. Frame times under 4ms!",
        "Short one.",
        "Multi-line post\n\nwith a blank line in between,\nand a third line.",
        "Emoji test 🦀🚀👩‍👩‍👧 and CJK 日本語のテキスト and RTL שלום.",
        "A longer post that goes on for a while to check wrapping inside a narrow column: the quick brown fox jumps over the lazy dog, again and again, until the text needs at least four or five lines to fit.",
        "Hello from the mock backend.",
    ];
    if n % 4 == 2 {
        // Rich post: mention + hashtag + link.
        let mention = &authors[(n + 2) % authors.len()];
        let m = format!("@{}", mention.handle);
        let text = format!("{m} have you seen #gpui yet? https://gpui.rs is great");
        let tag_start = m.len() + " have you seen ".len();
        let link_start = text.find("gpui.rs").map_or(0, |i| i - "https://".len());
        let spans = vec![
            Span {
                range: 0..m.len(),
                kind: SpanKind::Mention(UserRef {
                    id: Some(mention.id.clone()),
                    handle: mention.handle.clone(),
                }),
            },
            Span {
                range: tag_start..tag_start + "#gpui".len(),
                kind: SpanKind::Hashtag { tag: "gpui".into() },
            },
            Span {
                range: link_start..link_start + "https://gpui.rs".len(),
                kind: SpanKind::Link { url: url("https://gpui.rs/") },
            },
        ];
        RichText::new(text.clone(), spans).unwrap_or_else(|_| RichText::plain(text))
    } else {
        RichText::plain(PLAIN[n % PLAIN.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> FeedSource {
        FeedSource::Home { mode: crate::HomeMode::Following }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn following_is_recorded_and_shows_on_the_profile() {
        let b = MockBackend::new();
        let open = b.lock().authors.iter().skip(1).find(|a| !a.badges.protected).cloned().unwrap();
        let handle = open.handle.to_string();
        let status = block_on(b.set_following(&open.id, FollowAction::Follow)).unwrap();
        assert_eq!(status, FollowStatus::Following);
        assert!(block_on(b.profile(&handle)).unwrap().viewer.following);
        let status = block_on(b.set_following(&open.id, FollowAction::Unfollow)).unwrap();
        assert_eq!(status, FollowStatus::NotFollowing);
        assert!(!block_on(b.profile(&handle)).unwrap().viewer.following);
        assert_eq!(
            b.follows(),
            [(open.id.clone(), FollowAction::Follow), (open.id.clone(), FollowAction::Unfollow)]
        );
    }

    #[test]
    fn following_a_protected_account_is_a_request() {
        let b = MockBackend::new();
        let locked = b.lock().authors.iter().find(|a| a.badges.protected).cloned().unwrap();
        let status = block_on(b.set_following(&locked.id, FollowAction::Follow)).unwrap();
        assert_eq!(status, FollowStatus::Requested);
        assert!(!block_on(b.profile(&locked.handle)).unwrap().viewer.following);
    }

    #[test]
    fn first_page_has_page_size_and_older_cursor() {
        let b = MockBackend::new();
        let page = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        assert_eq!(page.items.len(), MockBackend::PAGE_SIZE);
        assert!(page.older.is_some());
        assert!(page.newer.is_some());
    }

    #[test]
    fn paging_older_walks_whole_feed_without_duplicates() {
        let b = MockBackend::new();
        let mut keys = std::collections::HashSet::new();
        let mut page = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        loop {
            for item in &page.items {
                assert!(keys.insert(item.key()), "duplicate {}", item.key());
            }
            match page.older.clone() {
                Some(t) => page = block_on(b.feed(&home(), PageRequest::Older(t))).unwrap(),
                None => break,
            }
        }
        assert_eq!(keys.len(), MockBackend::FEED_LEN);
    }

    #[test]
    fn newer_returns_only_pushed_posts() {
        let b = MockBackend::new();
        let first = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        let pushed = b.push_new(&home(), 3);
        let newer =
            block_on(b.feed(&home(), PageRequest::Newer(first.newer.clone().unwrap()))).unwrap();
        let got: Vec<_> =
            newer.items.iter().flat_map(|i| i.posts()).map(|p| p.id.clone()).collect();
        let mut expected = pushed;
        expected.reverse();
        assert_eq!(got, expected, "newest first");
        // Asking again with the new cursor yields nothing.
        let again = block_on(b.feed(&home(), PageRequest::Newer(newer.newer.unwrap()))).unwrap();
        assert!(again.items.is_empty());
    }

    #[test]
    fn user_feeds_hold_that_users_posts_and_media() {
        let b = MockBackend::new();
        let user = block_on(b.viewer()).unwrap();
        let feed = |tab| {
            let source =
                FeedSource::User { id: user.id.clone(), handle: user.handle.to_string(), tab };
            block_on(b.feed(&source, PageRequest::First)).unwrap().items
        };
        let posts = feed(UserTab::Posts);
        let own: Vec<_> = posts
            .iter()
            .filter_map(|i| if let FeedItem::Post(p) = i { Some(p) } else { None })
            .collect();
        assert!(!own.is_empty());
        assert!(own.iter().all(|p| p.author.id == user.id));
        let media = feed(UserTab::Media);
        assert!(!media.is_empty());
        assert!(media.iter().all(|i| matches!(i, FeedItem::Post(p)
            if p.author.id == user.id && !p.media.is_empty())));
    }

    #[test]
    fn injected_failure_is_returned_once() {
        let b = MockBackend::new();
        b.fail_next(BackendError::Network("down".into()));
        assert!(block_on(b.feed(&home(), PageRequest::First)).is_err());
        assert!(block_on(b.feed(&home(), PageRequest::First)).is_ok());
    }

    #[test]
    fn like_is_reflected_in_later_pages() {
        let b = MockBackend::new();
        let page = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        let id = page.items[0].posts()[0].id.clone();
        block_on(b.act(PostAction::Like(id.clone()))).unwrap();
        let page = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        assert!(page.items[0].posts()[0].viewer.liked);
        assert_eq!(b.actions(), vec![PostAction::Like(id)]);
    }

    #[test]
    fn generated_data_covers_every_item_shape() {
        let b = MockBackend::new();
        let mut page = block_on(b.feed(&home(), PageRequest::First)).unwrap();
        let mut all = page.items.clone();
        while let Some(t) = page.older.clone() {
            page = block_on(b.feed(&home(), PageRequest::Older(t))).unwrap();
            all.extend(page.items.clone());
        }
        let has = |f: &dyn Fn(&FeedItem) -> bool| all.iter().any(f);
        assert!(has(&|i| matches!(i, FeedItem::Repost { .. })));
        assert!(has(&|i| matches!(i, FeedItem::Thread(_))));
        assert!(has(&|i| matches!(i, FeedItem::Unavailable { .. })));
        let posts: Vec<&Post> = all.iter().flat_map(|i| i.posts()).collect();
        assert!(posts.iter().any(|p| p.media.len() == 4));
        assert!(posts.iter().any(|p| matches!(p.embed, Some(Embed::Poll(_)))));
        assert!(posts.iter().any(|p| matches!(p.embed, Some(Embed::Quote { .. }))));
        assert!(posts.iter().any(|p| matches!(p.embed, Some(Embed::Link(_)))));
        assert!(posts.iter().any(|p| !p.body.spans().is_empty()));
    }

    #[test]
    fn notifications_feed_has_every_kind_and_targets() {
        let b = MockBackend::new();
        let source = FeedSource::Notifications { filter: crate::NotificationFilter::All };
        let page = block_on(b.feed(&source, PageRequest::First)).unwrap();
        let notifs: Vec<&Notification> = page
            .items
            .iter()
            .filter_map(|i| match i {
                FeedItem::Notification(n) => Some(n),
                _ => None,
            })
            .collect();
        for kind in [
            NotificationKind::Like,
            NotificationKind::Repost,
            NotificationKind::Follow,
            NotificationKind::Quote,
        ] {
            assert!(notifs.iter().any(|n| n.kind == kind), "missing {kind:?}");
        }
        for n in &notifs {
            assert!(!n.actors.is_empty());
            assert_eq!(n.target.is_none(), n.kind == NotificationKind::Follow);
        }
        assert!(page.items.iter().any(|i| matches!(i, FeedItem::Post(_))), "mentions as posts");
    }
}
