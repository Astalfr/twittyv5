# Twitty — Implementation Plan

A native desktop client for X (Twitter), written in Rust with **GPUI** (via **GPUI Kit**). It talks to X's internal web GraphQL API (`https://x.com/i/api/graphql/<queryId>/<Operation>`) and looks like the new **X Pro / TweetDeck**: a multi-column deck.

> Status: **M0–M7 done**. Every column type loads from X; "Add column" dialog, per-column tabs, several decks. Next: M8 (like/repost/bookmark/reply).
>
> Living documentation is in [docs/](docs/README.md) and contributor/agent rules are in [CLAUDE.md](CLAUDE.md). Every milestone below is done only when it meets the definition of done in [docs/testing.md](docs/testing.md).

---

## 1. Research summary

### 1.1 GPUI

- GPUI is Zed's GPU-accelerated UI framework ([gpui.rs](https://www.gpui.rs/), [README](https://github.com/zed-industries/zed/tree/main/crates/gpui)).
- Core concepts:
  - **`Entity<T>`**: state owned by GPUI, accessed through contexts (`App`, `Context<T>`, `Window`).
  - **Views**: entities that implement `Render`. `render(&mut self, &mut Window, &mut Context<Self>) -> impl IntoElement`.
  - **Elements**: `div()` with a Tailwind-style builder (`flex`, `gap_2`, `bg`, `border_1`, …). `img()` takes local or remote images (remote needs an HTTP client set on `App`).
  - **Lists**: `uniform_list` (all rows the same height), and `list()` + `ListState` (rows of different heights, measured lazily). **Twitty uses `list()`**, because tweet heights vary.
  - **Actions** handle keybindings. The **executor** is GPUI's own (`cx.spawn`, `cx.background_spawn`), **not tokio**.
  - **Windows**: Win32 + DirectWrite, with no extra features needed.
- The `gpui` 0.2.2 crate on crates.io is stale. The ecosystem now ships **`gpui-pre-*` snapshots** of Zed's crates, and each snapshot may break the API.

### 1.2 GPUI Kit (longbridge/gpui-component)

- Repo: [longbridge/gpui-component](https://github.com/longbridge/gpui-component). Latest is **0.8.0** (2026-10-19).
- Crates:
  - **`gpui-kit`**: the one crate apps depend on. It re-exports gpui, `gpui-base`, `gpui-component` and assets, and **pins `gpui-pre =0.3.9`**.
  - **`gpui-base`**: behaviour without styling (virtual list, focus, popups, text selection, motion, …).
  - **`gpui-component`**: the styled components (70+ of them).
  - Also `gpui-webview` (macOS/Linux only, **not on Windows**), `gpui-shell` and `story` (the component gallery).
- Components useful for Twitty:
  - `avatar`, `button`, `tab`, `sidebar`, `input`, `popover`, `menu`, `dialog`, `sheet`
  - `notification`, `tooltip`, `skeleton`, `spinner`, `scroll`, `resizable`, `dock`
  - `text` (markdown/HTML), `badge`, `tag`, `switch`, `select`, `setting`
- Theming: JSON themes (`themes/*.json` + `.theme-schema.json`).
- Startup:
  ```rust
  gpui_kit::application().run(|cx| {
      gpui_kit::init(cx);
      cx.set_http_client(Arc::new(reqwest_client::ReqwestClient::user_agent("twitty")?)); // for img()
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|cx| TwittyApp::new(cx)))?;
  });
  ```
- `v_virtual_list` needs every row height up front, so it doesn't suit tweets. Use GPUI's `list()`/`ListState`, as gpui-component's diff view does.

### 1.3 X internal API

X doesn't document this API. These notes come from community clients: [`bun_x`](https://crates.io/crates/bun_x) (Rust, Apache-2.0, 2026-10-23, the freshest reference), [x-go](https://github.com/teslashibe/x-go) and [twitter-graphql-scraper](https://github.com/she-llac/twitter-graphql-scraper).

- **Auth**:
  - Cookies: `auth_token` (the session) and `ct0` (CSRF). The `x-csrf-token` header must equal `ct0`.
  - `authorization: Bearer <public web bearer>`. The bearer is built into x.com's JS and is the same for every user.
  - `x-twitter-auth-type: OAuth2Session`, `x-twitter-active-user: yes`, `x-twitter-client-language: en`.
  - `origin`/`referer: https://x.com`, a realistic Chrome UA, and stable `x-client-uuid` / `x-twitter-client-deviceid` values.
  - X rotates cookies through `Set-Cookie`, and we must keep the new values.
- **Query IDs** change with every X release (about every 2–4 weeks). To find them:
  1. Fetch `x.com` pages.
  2. Collect the `abs.twimg.com/responsive-web/client-web/*.js` bundle URLs.
  3. Regex `{queryId, operationName}` out of the bundles.
  4. Cache the results on disk for 24h, falling back to an embedded snapshot.
- **`features` / `fieldToggles`**: JSON query params, one set per operation. They also drift.
- **`x-client-transaction-id`**: enforcement is increasing. For now a random value is accepted. Error code 353 means X now requires the real algorithm (port it from x-go `transaction.go` / the `x-client-transaction` crate).
- **Rate limits**: track the `x-rate-limit-limit/remaining/reset` headers per endpoint, and back off on 429.
- **Response shape**: `data.<root>.timeline.instructions[]`, with these parts:
  - Instruction types: `TimelineAddEntries`, `TimelineReplaceEntry`, `TimelinePinEntry`, `TimelineClearCache`, …
  - Entry types: `TimelineTimelineItem` / `TimelineTimelineModule` / `TimelineTimelineCursor`.
  - Result typenames: `Tweet` / `TweetWithVisibilityResults` / `TweetTombstone` / `TweetUnavailable`.
  - Inside a tweet: `legacy`, `core.user_results`, `note_tweet`, `quoted_status_result`, `retweeted_status_result`, `card`, `extended_entities`.
- **X Pro** ([help](https://help.x.com/en/using-x/x-pro)) has Decks (workspaces of columns), a column creator, top/latest ordering and video docking. It has been Premium+ only since March 2026. `bun_x` documents its deck GraphQL (ViewerAccountSync, CreateDeck, …), which could allow an optional sync later.

---

## 2. Core principle: Twitty owns its data model

**X's JSON structures never leave the X adapter.** X changes field names, nesting, typenames and feature flags all the time. If the UI, the cache or the settings used those shapes directly, every X change would ripple through the whole codebase and break saved data.

So Twitty is built as **ports and adapters**, with an anti-corruption layer:

```
   X GraphQL JSON  ──►  twitty-x::wire  ──►  twitty-x::translate  ──►  twitty-model  ◄──  twitty (UI)
   (unstable)           (private, lenient)     (the only place that       (stable, ours)
                                                knows both worlds)
```

### Rules

1. **`twitty-model`** is the stable domain crate. It depends only on `serde`, `chrono`, `url` and `smol_str`. It has no X field names and no `__typename`s. It is designed around what Twitty *displays*, not around what X *sends*.
2. **`twitty-x::wire`** is private (`pub(crate)`). It reads X's JSON **leniently**:
   - mostly `serde_json::Value` with small typed helpers
   - `#[serde(default)]` everywhere
   - unknown variants map to `Unknown`.
3. **`twitty-x::translate`** is the only code that converts wire data into model types. When X changes, only this module (and its fixtures) changes.
4. **What crosses the boundary**: the UI and the rest of the app see only `twitty-model` types and the `Backend` trait. `twitty-x` re-exports nothing from `wire`.
5. **Fail soft**: an entry that can't be translated becomes `FeedItem::Unsupported { reason }`, or is skipped with a `tracing::warn!`. It never fails the whole page.
6. **Opaque handles**: X cursors, query IDs and rest IDs are wrapped as opaque newtypes (`PageToken`, `PostId`, `UserId`). The UI never parses or builds them.
7. **Data saved to disk** (caches, decks, settings) uses the `twitty-model` types and a `schema_version` field, never raw X JSON. A change at X can't break saved data.
8. **Normalisation happens in `translate`**:
   - code-point entity offsets (verified live) → byte ranges in Rust `String`s
   - `t.co` → expanded URLs, with trailing media links stripped
   - HTML entities decoded
   - `note_tweet` long text preferred over the truncated text
   - retweet unwrapped
   - the best video variant chosen
   - X date strings → `DateTime<Utc>`.

### 2.1 Domain model sketch (`twitty-model`)

```rust
// ids.rs — opaque, cheap to clone
pub struct PostId(SmolStr);
pub struct UserId(SmolStr);
pub struct PageToken(String);          // wraps X cursors; meaningless outside the backend

// user.rs
pub struct Profile {
    pub id: UserId,
    pub handle: SmolStr,               // without '@'
    pub display_name: String,
    pub avatar: Option<ImageRef>,
    pub banner: Option<ImageRef>,
    pub bio: Option<RichText>,
    pub badges: Badges,                // verified kind, protected, affiliate...
    pub counts: ProfileCounts,         // followers, following, posts
    pub viewer: ViewerRelation,        // following, followed_by, blocking, muting
}

// post.rs
pub struct Post {
    pub id: PostId,
    pub author: Arc<Profile>,
    pub created_at: DateTime<Utc>,
    pub body: RichText,
    pub media: Vec<Media>,
    pub embed: Option<Embed>,
    pub reply_to: Option<ReplyRef>,    // post id + handle, for "Replying to @x"
    pub metrics: Metrics,              // replies, reposts, likes, quotes, views, bookmarks
    pub viewer: ViewerPostState,       // liked, reposted, bookmarked
    pub lang: Option<SmolStr>,
    pub source_url: Url,               // https://x.com/<handle>/status/<id>
}

pub struct RichText { pub text: String, pub spans: Vec<Span> }   // spans use byte ranges
pub struct Span { pub range: Range<usize>, pub kind: SpanKind }
pub enum SpanKind { Mention(UserRef), Hashtag(SmolStr), Cashtag(SmolStr), Link { display: String, url: Url } }

pub enum Media {
    Image { image: ImageRef, alt: Option<String> },
    Video { poster: ImageRef, stream: Option<Url>, duration: Option<Duration>, aspect: (u32, u32) },
    Gif   { poster: ImageRef, stream: Option<Url>, aspect: (u32, u32) },
}
pub struct ImageRef { pub url: Url, pub size: Option<(u32, u32)> }  // size hints let the list reserve height

pub enum Embed {
    Quote(Box<PostOrGone>),
    LinkCard { url: Url, title: String, description: Option<String>, image: Option<ImageRef>, domain: String },
    Poll(Poll),
}
pub enum PostOrGone { Post(Post), Unavailable { reason: UnavailableReason } }

// feed.rs — what a column displays
pub enum FeedItem {
    Post(Post),
    Repost { by: Arc<Profile>, post: Post },
    Thread(Vec<Post>),                       // conversation modules
    Notification(Notification),
    Unavailable { id: Option<PostId>, reason: UnavailableReason },
    Unsupported { reason: String },          // something we failed to translate
}
pub struct FeedPage { pub items: Vec<FeedItem>, pub newer: Option<PageToken>, pub older: Option<PageToken> }

pub enum FeedSource {                        // stored in decks, stable on disk
    Home { mode: HomeMode },                 // ForYou | Following
    Notifications { filter: NotifFilter },
    Search { query: String, sort: SearchSort },
    List { id: ListId },
    User { id: UserId, tab: UserTab },       // Posts | Replies | Media | Likes
    Bookmarks,
    Thread { root: PostId },
}
```

### 2.2 Backend port (`twitty-model::backend`)

```rust
#[async_trait]   // or explicit boxed futures
pub trait Backend: Send + Sync + 'static {
    async fn viewer(&self) -> Result<Profile, BackendError>;
    async fn feed(&self, source: &FeedSource, page: PageRequest) -> Result<FeedPage, BackendError>;
    async fn thread(&self, id: &PostId) -> Result<ThreadView, BackendError>;
    async fn profile(&self, handle: &str) -> Result<Profile, BackendError>;
    async fn act(&self, action: PostAction) -> Result<(), BackendError>;   // Like/Unlike/Repost/Bookmark...
    async fn publish(&self, draft: Draft) -> Result<Post, BackendError>;
}
pub enum PageRequest { First, Newer(PageToken), Older(PageToken) }
pub enum BackendError { Unauthorized, RateLimited { reset_at: DateTime<Utc> }, NotFound, Upstream(String), Network(String), Decode(String) }
```

The UI depends only on this trait. Other uses of it:
- a **`MockBackend`** with fixture data, for UI development offline and for `#[gpui::test]`
- other networks later if wanted (Bluesky, Mastodon).

---

## 3. Workspace layout

```
twittyv5/
├─ Cargo.toml                 # [workspace] resolver = "3", shared deps, profile tweaks
├─ Cargo.lock                 # committed (GPUI snapshots are pinned exactly)
├─ PLAN.md
├─ crates/
│  ├─ twitty-model/           # stable domain types + Backend trait (no I/O)
│  │  └─ src/{ids,user,post,rich_text,media,feed,notification,backend,error}.rs
│  ├─ twitty-x/               # X adapter, implements Backend
│  │  ├─ src/transport/       # reqwest client, headers, session, cookie rotation, txid, rate limits
│  │  ├─ src/discovery/       # queryId + features discovery from JS bundles, disk cache, embedded fallback
│  │  ├─ src/ops/             # one module per GraphQL op: builds variables/features, returns wire::Response
│  │  ├─ src/wire/            # PRIVATE lenient readers over X JSON (instructions, entries, results)
│  │  ├─ src/translate/       # wire → twitty-model (the only place that knows both)
│  │  ├─ src/backend.rs       # impl Backend for XBackend
│  │  ├─ src/bin/twitty-x-cli.rs   # dev CLI: dump a feed as model JSON
│  │  └─ tests/fixtures/      # captured + anonymised X responses, with model snapshots (insta)
│  └─ twitty/                 # GPUI app (binary)
│     ├─ src/main.rs
│     ├─ src/runtime.rs       # tokio runtime thread + bridge to GPUI tasks
│     ├─ src/state/           # AppState, DeckStore (persisted), PostStore (dedup cache by PostId)
│     ├─ src/ui/{rail,deck,column,post_cell,media_grid,quote_card,composer,thread_panel,profile_card,login,settings}.rs
│     ├─ src/actions.rs       # keybindings
│     └─ themes/{x-lights-out,x-dim,x-light}.json
```

Main dependencies:
- `gpui-kit = "=0.8.0"`, `gpui-pre-reqwest-client = "=0.3.9"` (for `img()`)
- `tokio`, `reqwest 0.13` (rustls, cookies, gzip/brotli), `serde`, `serde_json`, `chrono`, `url`, `smol_str`, `regex`
- `thiserror`, `tracing`, `keyring`, `directories`, `uuid`
- dev: `insta`, `wiremock`

### Async bridge

`twitty-x` runs on a **dedicated tokio runtime**, on its own thread, owned by `twitty::runtime`. UI code calls `runtime.spawn(backend.feed(..))` and awaits the `JoinHandle` inside `cx.spawn`, then updates entities. tokio's `JoinHandle` can be awaited from any executor, so this works. GPUI's own image loading uses the pinned GPUI reqwest client.

---

## 4. UI: X Pro / TweetDeck look

I can't access X Pro myself, so this is a spec rather than a copy. The colours below are from memory and must be **checked against x.com's live CSS**.

```
┌──┬───────────────────┬───────────────────┬───────────────────┬──────
│𝕏 │ ⌂ Home   For you ⚙│ 🔔 Notifications ⚙│ 🔍 "rust gpui"   ⚙│ ... (horizontal scroll)
│⌂ ├───────────────────┼───────────────────┼───────────────────┤
│🔍│ ◯ Name @h · 2m  ⋯ │ ♥ Alice liked ... │ ◯ ...             │
│🔔│ Body text with    │                   │                   │
│✉ │ @links #tags      │                   │                   │
│☰ │ ┌──────┬──────┐   │                   │                   │
│＋│ │ img  │ img  │   │                   │                   │
│  │ └──────┴──────┘   │                   │                   │
│  │ 💬12  🔁3  ♥40  📊1k│                  │                   │
│✎ │───────────────────│                   │                   │
│◯ │ ...               │                   │                   │
└──┴───────────────────┴───────────────────┴───────────────────┴──────
```

- **Left rail (~68px)**: logo, then Home, Explore/Search, Notifications, Messages (later), Lists, **Add column**, and the deck switcher. At the bottom: the Compose button and the account avatar menu.
- **Deck**: columns side by side in one horizontal scroll area. Each column is a fixed width (Narrow 310 / Medium 350 / Wide 400px) and can be reordered by dragging. Columns are separated by 1px borders.
- **Column header** (sticky): icon, title, optional subtitle (@handle or query), and a ⚙ popover (width, Top/Latest, media preview size, clear, remove).
- **Post cell**:
  - 40px round avatar
  - **name** · @handle · relative time (gray), and a ⋯ menu
  - body text, with links in the accent colour
  - media grid with 1–4 images and 16px rounded corners
  - quote card with a bordered, smaller version of the post
  - link card
  - action row (reply, repost, like, views, bookmark, share) in gray, with active colours (like = pink, repost = green).
- **Behaviour**:
  - **"↑ N new posts"** pill when the column is scrolled down
  - skeleton rows while loading
  - an inline "Show more" for long posts
  - clicking a post opens a **thread panel** (a side sheet).
- **Themes** (gpui-kit theme JSON):
  - Lights out: bg `#000000`, border `#2F3336`, text `#E7E9EA`, secondary `#71767B`, accent `#1D9BF0`
  - Dim: bg `#15202B`
  - Light: bg `#FFFFFF`
- **Font**: Segoe UI / system font. Chirp is X's own font and isn't bundled.
- **Video**: GPUI has no video player. v1 shows the poster image with a ▶ badge and duration, and opens the video in the browser or system player.

---

## 5. Milestones

| # | Milestone | Done when |
|---|---|---|
| **M0** | Toolchain check | gpui-kit 0.8.0's `hello_world` builds and runs on Windows 11 with Rust 1.98. We know whether a `build.rs` or Windows manifest is needed. |
| **M1** | Workspace + model | The 3 crates exist. `twitty-model` types and the `Backend` trait are defined. `MockBackend` serves fixture posts. |
| **M2** | UI skeleton on mock data | Rail, deck, three columns with `list()`/`ListState`, post cells, the Lights-out theme. Everything driven by `MockBackend`. |
| **M3** | X transport | Session, static headers, txid (random), cookie rotation, rate-limit tracker, queryId/features discovery and cache. `twitty-x-cli` dumps raw HomeTimeline JSON. |
| **M4** | Wire + translate | Lenient wire readers and translation into the model. At least 15 fixtures (retweet, quote, quote-of-deleted, thread module, long note, 4 images, video, GIF, poll, card, tombstone, withheld, pinned, cursors), with `insta` snapshots of the **model** output. |
| **M5** | Real Home column | `XBackend` is plugged into the UI. Paging with older/newer tokens, image loading, `PostStore` deduplication. *(Done: no separate `PostStore`, see [decisions.md](docs/decisions.md).)* |
| **M6** | Login + persistence | Cookie dialog (paste `auth_token` + `ct0`, with how-to steps), checked by `viewer()`, stored in keyring. Decks and settings saved in `%APPDATA%\Twitty` with `schema_version`. |
| **M7** | All column types | Notifications, Search, List, User, Bookmarks, Thread. Add/remove/reorder/resize columns. Several decks. |
| **M8** | Interaction | Like, repost, bookmark (optimistic updates with rollback), reply/compose (`publish`), thread panel, profile hover card, open in browser. |
| **M9** | Polish | Per-column polling within rate limits, new-posts pill, keyboard actions (j/k, l, r, t, n, Esc), image lightbox, Dim/Light themes, settings page. |
| **M10** | Hardening | Real transaction-id algorithm if needed, drift detection (a counter of `Unsupported`/decode warnings shown in a debug panel), logging, and an optional X Pro deck sync. |

---

## 6. Testing strategy

- **twitty-x**: fixture-driven tests. A raw X JSON response goes through translate, and the resulting `FeedPage` is checked with `insta` snapshots. When X changes, we capture a new fixture and fix `translate`, and the model snapshots must stay the same.
- **transport**: `wiremock` tests for headers, cookie rotation, 429 backoff and recovery from stale query IDs (404 → refresh IDs → retry once).
- **UI**: `#[gpui::test]` with `MockBackend` (paging, optimistic like/rollback, deck persistence).
- **Manual**: `twitty-x-cli` runs against a real (secondary) account.

---

## 7. Risks

- **Terms of service and account bans**: unofficial API use breaks X's rules. Develop with a secondary account.
- **queryId / features drift**: handled by runtime discovery, the cache and the embedded fallback. The domain model keeps the rest of the app isolated from it.
- **Schema drift**: limited to `wire` + `translate` by design. Lenient decoding plus `Unsupported` items mean the UI degrades instead of crashing.
- **Transaction ID enforcement** (error 353): port the real algorithm when it's required.
- **Bot / TLS fingerprinting**: if plain reqwest gets blocked, swap the transport for a Chrome-impersonating client (`wreq`). This change stays inside `transport`.
- **GPUI API churn**: exact pins. Upgrade only when a new gpui-kit is released, in a separate commit.
- **No embedded webview on Windows**: login means pasting cookies. A future option is a small external helper that reads them from the user's browser profile.

---

## 8. Sources

- GPUI: https://www.gpui.rs/ · https://github.com/zed-industries/zed/tree/main/crates/gpui
- GPUI Kit / gpui-component: https://github.com/longbridge/gpui-component
- bun_x (reference implementation, Apache-2.0): https://crates.io/crates/bun_x
- x-go: https://github.com/teslashibe/x-go
- twitter-graphql-scraper: https://github.com/she-llac/twitter-graphql-scraper
- x-client-transaction (Rust): https://crates.io/crates/x-client-transaction
- X Pro help: https://help.x.com/en/using-x/x-pro · TweetDeck history: https://en.wikipedia.org/wiki/TweetDeck
