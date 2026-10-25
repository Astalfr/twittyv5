# Architecture

## Crates

```
                ┌──────────────────────────┐
                │        twitty (bin)      │  GPUI app: views, state, runtime bridge
                │  depends on dyn Backend  │
                └────────────┬─────────────┘
                             │ uses
             ┌───────────────┴───────────────┐
             ▼                               ▼
┌──────────────────────────┐    ┌──────────────────────────┐
│       twitty-model       │◄───│        twitty-x          │  X adapter: impl Backend
│ domain types + Backend   │    │ transport · discovery ·  │
│ no I/O, no GPUI, no X    │    │ ops · wire · translate   │
└──────────────────────────┘    └──────────────────────────┘

twitty also uses twitty-media (video decoding, Media Foundation), which
depends on nothing else of ours.
```

| Crate | Responsibility | May depend on | Must not depend on |
|---|---|---|---|
| `twitty-model` | Stable domain types (`Post`, `Profile`, `FeedItem`, …), `Backend` trait, `BackendError`, `MockBackend` (feature `mock`) | serde, chrono, url, smol_str, thiserror | gpui, reqwest, tokio, twitty-x |
| `twitty-x` | Everything X-specific: HTTP, auth, query-ID discovery, GraphQL ops, wire parsing, translation into the model | twitty-model, reqwest, tokio, serde_json, regex | gpui, twitty |
| `twitty-media` | Video decoding: `Decoder` opens a file or URL and returns BGRA frames and float sound (`Packet`s in file order) with their times, and seeks exactly (frames and sound before the target are skipped). Media Foundation on Windows, `Unsupported` elsewhere. The only crate with `unsafe` (COM), confined to its `mf` module | thiserror, windows | gpui, twitty-model, everything Twitter |
| `twitty` | UI, app state, persistence of decks and settings, keyring, runtime bridge | twitty-model, twitty-x, twitty-media, gpui-kit | (Twitter JSON types, which aren't public anyway) |

`twitty` uses `twitty-x` only in `main.rs`, to find a session, build an `XBackend` and hand it over as `Arc<dyn Backend>`. Without a session, `main.rs` uses `MockBackend` and shows a "Demo mode" banner.

`twitty` is a library plus a thin binary:

| Module | Contents |
|---|---|
| `state::column` | `ColumnModel`: paging, cursors, de-duplication, unseen count, `ListChange` instructions for the view. It keeps every loaded row and shows those the column's filters let through; all its indexes count shown rows |
| `state::filters` | `ColumnFilters`: which kinds of tweets a column shows (`TweetKind`: text only, images, videos, replies, quotes, retweets) and how a row is classified |
| `state::interactions` | Optimistic like/repost/bookmark overrides with rollback |
| `state::repost` | What the retweet dialog offers (retweet or undo) from the viewer's effective state |
| `state::follow` | The profile sheet's Follow button: optimistic follow/unfollow, rollback with the reason, protected accounts' requests, late profile refreshes ignored after a click |
| `state::deck` | `DeckFile` (versioned `decks.json`), column add/remove/move, `ColumnSettings` (name, width, alerts, filters) |
| `state::alerts` | Which loads announce new rows, and the notification: who it's from (title and picture) and its lines |
| `state::media_layout` | Geometry of the 1–4 media grid, and which part of an image a tile shows (`crop_for`) |
| `state::profile` | Profile sheet: Tweets / Media tabs and their sources, media gallery tiles, list row mapping and splices |
| `state::sheets` | `SheetStack`: the stacked side sheets (threads, profiles), how far left each lower one shifts, which are drawn, and where each moved from (for its slide) |
| `state::motion` | Animation timing: which rows just arrived (`Recent`, `Entrance`), row frames, the toggle "pop" curve ([ui.md](ui.md#animations)) |
| `state::playback` | Video frame timing (`Timeline`): the clock starts with the first frame, waits for a slow decoder, pauses, follows the sound while it plays (`tick_with`), seeks (with a preview while paused) and ends; which frame is due and which leave the screen |
| `video` | `Video` global and `VideoSource` trait. `open` returns a `Playback`: the event receiver and a `Control` (seek with a generation, pause, volume, the sound's clock). `SystemVideo` decodes each playback on its own thread with `twitty_media`: frames go to the view through a bounded channel, sound straight to the output device. The thread never blocks without watching its commands (a seek reaches it while the view is paused; a view that goes away stops it). Loops keep the clock growing |
| `audio` | `AudioShared` (the sound queue, the generation of the latest seek, pause, gain, the clock) and `AudioOut`, the rodio source playing it on the audio thread: one lock per chunk, silence without consuming while paused or dry |
| `format` | Relative times and compact counters |
| `runtime` | `Runtime::Tokio` (production) / `Runtime::Inline` (tests) |
| `state::update` | Self-update decisions: `Version` (parse, compare), which release asset is the installer (`twitty-setup-<version>-x64.exe`, with GitHub's SHA-256 `digest`), and the banner's `Updates` state (available, downloading, failed, dismissed for the session) |
| `update` | Self-update I/O behind `UpdateServices` (`ReleaseSource` + `Launcher`): `GitHubReleases` reads `/releases/latest` from the public API and downloads the installer to the cache dir, hashing it as it streams (a mismatch deletes it); `launch_installer` runs it silently. `main.rs` provides the services only when `unins000.exe` sits next to the exe ([development.md](development.md#releases-and-self-update)) |
| `ui::app` | Root view: rail + horizontally scrolling deck, persists deck changes |
| `ui::column` | Column view: header, `list()` + `ListState`, footer row, polling, "new tweets" pill |
| `ui::post` | Row rendering: tweets, retweets, threads, media, quotes, link cards, polls, action bar |
| `ui::player` | `VideoPlayer` view: pulls frames of the current generation into its `Timeline` on every draw (on the sound's clock), frees frames that leave the screen from the sprite atlas. `Options::GIF` draws pictures only; `Options::video` adds the control bar (sliders seek on release; `VideoVolume` global for the session). The container sets how frames are drawn (`set_draw`) and handles clicks on the picture |
| `ui::gif` | GIFs playing in timeline tiles: `GifTile` keyed state (`window.use_keyed_state`, so it lives only while the tile is drawn), the hover play/pause button, the `GifAutoplay` global set from the settings |
| `ui::hub` | `InteractionsHub` entity shared by all columns; sends actions, rolls back, emits `ActionFailed` |
| `ui::theme`, `ui::assets` | Bundled themes (`themes/x.json`) and the embedded Lucide icon subset |

## Layers inside `twitty-x`

```
ops/HomeTimeline.rs ──► transport (headers, cookies, txid, rate limit) ──► HTTPS
        │                         ▲
        │                discovery (queryId + features cache)
        ▼
   serde_json::Value ──► wire (lenient readers, pub(crate)) ──► translate ──► twitty_model::FeedPage
```

- **transport**: one `reqwest::Client`, static headers, per-request credentials, cookie rotation, per-endpoint rate-limit tracking, retries.
- **discovery**: resolves `operationName → (queryId, features)`. The order is runtime cache → refresh from Twitter's JS bundles → embedded fallback.
- **ops**: one module per GraphQL operation. Each builds `variables`/`features`/`fieldToggles` and returns raw JSON.
- **wire**: walks Twitter's JSON (instructions, entries, results). It tolerates missing fields and unknown typenames.
- **translate**: the only code that knows both Twitter's shapes and the model. All normalisation happens here.

## Async model

GPUI runs its own executor on the main thread, and reqwest needs tokio. So:

1. `Runtime::tokio()` starts a multi-threaded **tokio runtime on background threads**. It's stored as a GPUI global.
2. A view calls `Runtime::global(cx).spawn(async move { backend.feed(&source, page).await })`, which returns a future any executor can await.
3. The view awaits it inside `cx.spawn(async move |this, cx| …)`.
4. Tests register `Runtime::inline()` instead. It polls the backend future on GPUI's test executor, so `run_until_parked()` stays deterministic.
5. When the result arrives, the task updates the entity through its `WeakEntity` and calls `cx.notify()`.

Images use GPUI's own loader, with `cx.set_http_client(gpui-pre-reqwest-client)` set at startup.

## Data flow example: opening the Home column

1. `DeckView` creates a `ColumnView` for `FeedSource::Home { mode: Following }`.
2. `ColumnView` asks the backend for `PageRequest::First` through the runtime bridge, and shows skeleton rows meanwhile.
3. `XBackend::feed` picks the operation and variables for the source (`requests::feed`: `HomeLatestTimeline`, cursor if any), sends the request through `XClient::graphql` and translates the response with `translate::feed_page`.
4. `ColumnModel` drops rows it already has (by `FeedItem::key`), stores the cursors and returns a `ListChange`. `ColumnView` applies it with `ListState::reset`/`splice` and calls `cx.notify()`. Viewer state shared between columns (likes, retweets, bookmarks) lives in `InteractionsHub`, keyed by `PostId`.
5. Scrolling near the bottom triggers `PageRequest::Older(token)`. The "new tweets" poll uses `PageRequest::Newer(token)`.

## App state (`twitty` crate)

- `TwittyApp` (root view) owns the `DeckFile`, the column views and the shared `InteractionsHub`.
- Each `ColumnView` owns its `ColumnModel` (rows + cursors) and its `ListState`. The list has one extra footer row (spinner, retry, "caught up").
- The viewer's likes, retweets and bookmarks are overrides in `InteractionsHub`, so every column shows the change instantly. Failures roll back.
- Tweet rows don't know about panels or dialogs: they emit `hub::Navigate` (`Thread`, `Reply`, `Repost`, `Profile`, `Mention`, `UserColumn`, `Media`) through the hub, and `TwittyApp` opens the thread panel, the compose dialog, the retweet dialog (whose Quote opens compose), the profile sheet, a User column or the image viewer. The thread panel and the profile sheet are stacked side sheets (`Sheet` in `ui/app.rs`, kept in a `state::sheets::SheetStack`).
- Decks are saved as JSON in `decks.json` in the config folder (`%APPDATA%\Twitty\config`, or `dev-data\config` in debug builds: `twitty::dirs`) (atomic write), with a `schema_version`. Saves are chained (`TwittyApp::persist`), so quick edits never interleave their writes.
- New-post alerts: a column checks `state::alerts::announce` after each load. The sound goes through the `sound::Sound` global (`SystemSound` from `main.rs`; no global in tests means silence, `RecordingSound` records). System notifications go through `notifier::show`: with the `notifier::Notifications` global (`WindowsToasts` from `main.rs`), Twitty builds the Windows toast itself so it can carry the author's avatar, downloaded once with `cx.http_client()` into the cache folder (`avatars/<hash of the URL>.jpg`); a failed download shows the toast without it. `WindowsToasts::new` registers the app id (`Twitty.Twitty`, the one `main.rs` gives `cx.set_app_identity`) for the current user, as GPUI does, plus its icon: `IconUri` points at the logo it saves as `twitty.png` in the cache folder (GPUI only writes the name, which gives toasts a generic icon). Without the global (tests by default, other platforms) the toast goes through GPUI's `show_system_notification`, without a picture; `RecordingNotifier` records toasts in tests.
- Inline GIFs: `ui::post::media_grid` adds `ui::gif::inline_gif` over each GIF tile, keyed by the tweet and media index inside `RowCtx::scope` (one per list: a column, the thread panel, the profile sheet), so the same tweet in two columns plays separately. Whether it plays is `state::playback::gif_plays` (the viewer's choice, else the autoplay setting).
- Video: the image viewer creates a `VideoPlayer` when it shows a GIF or a video (`TwittyApp::sync_media_player`) and drops it when it steps away or closes; dropping it hangs up the channel, which stops the decoder thread. Without a `video::Video` global (tests, by default) the player fails at once and the viewer keeps the poster and "Play on Twitter"; `FakeVideo` stands in for tests.
- Credentials (`twitty::account`): the sign-in dialog stores the session in Windows Credential Manager (`keyring`, entry `Twitty` / `x-session`, or `x-session-dev` in debug builds, a versioned JSON payload). `main.rs` gives the app an `AccountServices` (a `Connector` that turns `Credentials` into an `Arc<dyn Backend>`, plus the `CredentialStore` and, on Windows, the `BrowserSignIn` window from `twitty::browser_login`), so the UI never sees `twitty-x`. Startup order and sign-out rules: "Signing in" in [x-api.md](x-api.md).
- Settings (`state::settings`): `settings.json` next to `decks.json`, versioned; it holds the theme and the polling, animations and update-check switches.
- Signing in or out replaces the backend: `TwittyApp` rebuilds the hub and every column (old views are dropped, which cancels their loads and polling).
