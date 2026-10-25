# Decision log

Lightweight ADRs: newest first. Each entry: date, decision, why, consequences. Add one whenever a choice isn't obvious from the code, or when Twitter forces a change.

---

### 2026-10-24: Self-update from GitHub Releases, unsigned
- **Decision**: An installed copy checks the repository's latest GitHub Release (public API, no token) and, on the user's click, downloads its Inno Setup installer, checks it against GitHub's SHA-256 `digest`, runs it silently and quits; the installer restarts Twitty. CI publishes a release for each `v*` tag, only after tests and build pass. Nothing installs without a click.
- **Why**: The repository is public, so releases need no credentials, and the installer already handles upgrades (same `AppId`, previous folder and install mode, closing a running copy). GitHub's own digest avoids a checksum file of our own. Only copies installed by the installer update, so a `cargo run` build never replaces itself.
- **Consequences**: There's no signature (no minisign key, no code-signing certificate): the digest comes from the same place as the file, so it catches corruption, not a compromised account or repository. Signing releases would be the next step. Unauthenticated API calls are limited to 60 an hour per IP; a check every 6 hours stays far below. Pre-releases are never offered. The banner's "Later" lasts for the session.

### 2026-10-24: Windows installer built with Inno Setup
- **Decision**: CI builds a per-user Inno Setup installer (`installer/twitty.iss`) next to the raw exe on every run.
- **Why**: Twitty is a single self-contained exe, so the installer only needs to copy it, add shortcuts and register an uninstaller. Inno Setup does that in one short script, installs without admin rights, and closes a running app on upgrade. WiX/MSI would need a heavier toolchain and authoring for no gain here.
- **Consequences**: The version comes from the workspace `Cargo.toml`; bumping it is what makes an installer an upgrade. The installer and exe are unsigned (SmartScreen warns). User data survives uninstall. Publishing GitHub Releases from tags isn't set up.

### 2026-10-24: Community Notes are read in the thread panel
- **Decision**: A tweet's Community Note is shown in full only under the tweet the thread panel is about. Everywhere else (columns, profiles, the panel's other tweets, quote cards) a compact alert under the text, "Readers added context", says there is one; clicking it, like the rest of the row, opens the panel.
- **Why**: A note is often longer than the tweet and would crowd narrow columns. A one-line alert under the text warns the reader where they read (an icon in the header was too easy to miss), and the full note is one click away.
- **Consequences**: `RowCtx` carries the panel's focus (`None` elsewhere), and `state::community_note::display` picks marker or full note. Only notes Twitter sends in `birdwatch_pivot` (rated helpful) are shown; `has_birdwatch_notes` alone shows nothing. The note's title and "Rate it on Twitter" are in English like the rest of the UI; Twitter's localized `title` is not used. Rating notes isn't supported: the link opens the note's page on x.com.

### 2026-10-24: Grok translations are shown by default
- **Decision**: Turn on `responsive_web_grok_show_grok_translated_post` and show a tweet's translation instead of its text when Twitter sends one, with "Translated from <language> · Show original" above it. The switch is per tweet, shared by every column, and not saved.
- **Why**: That's what x.com does, and a translation can't be mistaken for the author's words because the note is always there. Keeping the choice for the session only avoids a persisted set of tweet ids.
- **Consequences**: Translations are into English (`destination_language` `en`, like our `x-twitter-client-language`); Twitter leaves tweets in the account's own languages untranslated. Translating into another language would mean sending another client language, which hasn't been tried and would also change the English tombstone texts `translate::tweet` matches. The note is in English like the rest of the UI. Notification rows still show the original text of their target.

### 2026-10-24: Follow goes through v1.1 REST, and some 403s aren't a lost session
- **Decision**: `Backend::set_following` POSTs a form to `1.1/friendships/create.json` / `destroy.json`. The transport gained a form body next to JSON. A 403 carrying 158/160/161/162 becomes `XError::Api`, not `Unauthorized`.
- **Why**: Twitter's web client follows over REST; there is no GraphQL follow mutation in its bundles. Those 403s mean Twitter refused the follow (blocked, limit, already requested); treating them as an expired session would open the sign-in dialog.
- **Consequences**: A pending request to a protected account shows "Requested" until the panel closes; a reopened profile shows "Follow" again (the model has no `follow_requested` yet), and clicking it is answered with 160, which reads as `Requested`. Cancelling a request isn't offered.

### 2026-10-24: Videos play in the viewer, timed by their sound
- **Decision**: Videos play in the media viewer with sound and a control bar. The decoder thread reads the video and audio tracks together (`ReadSample` on any stream) and queues the sound straight into a rodio source on the default output device; frames go to the view. The sample being played is the master clock: the view sets its `Timeline` to it on every draw. When no sound comes out (no track, no device, its end, a dry queue, or a device that stopped pulling samples for 250 ms, such as unplugged headphones), the wall clock carries on from the last position the sound gave; only when both picture and sound are dry does the clock wait. A seek bumps a generation: the sound queue is cleared at once, the thread seeks (exactly: frames and sound before the target are skipped, the sound trimmed in whole frames so the channels keep their order), and everything older is dropped. Seeking happens on slider release. Volume and mute are kept for the session in a global, not saved. Every MP4 variant reaches the model (best first); the quality menu reopens the playback on another file and seeks it to the same point, and the choice isn't remembered for the next video.
- **Why**: The output device's clock is the one the ear follows, and resampling, device latency and drift all stay on its side. A clock that stopped whenever the sound did would freeze the picture on a common short sound track, or deadlock when a file interleaves coarsely (the thread blocked sending frames can't decode the sound the clock waits for). Each seek restarts a network download, so dragging doesn't seek.
- **Consequences**: The decoder thread polls its commands every 5 ms while it waits (on a full frame channel, or with 2 s of sound queued), so seeks reach it while the view is paused and a closed viewer stops it. Dropping the control also silences the sound at once, even while the thread is still blocked in a download. A seek to the very end (no frame left) ends the video, so Play starts it over. Without an output device a video plays silently on wall time. Sound is about 200 ms late to start (the device opening). Timeline video tiles still open the viewer rather than play in place.

### 2026-10-24: Timeline GIFs play on demand, autoplay is opt-in
- **Decision**: GIF tiles in rows play in place. By default they wait for the play/pause button shown on hover; a setting ("Play GIFs automatically", off by default) starts every GIF whose row is drawn. The button's choice beats the setting for that tile. Each tile's state is GPUI keyed state, so a row that leaves the list (scrolled away) drops its player and stops its decoder; coming back starts it over, and forgets a pause.
- **Why**: Each playing GIF is a download plus a decoder thread and a texture per frame. Off by default keeps a scrolling deck cheap and quiet, and the button covers the occasional GIF. Keyed state ties the cost to what is on screen with no bookkeeping of our own.
- **Consequences**: The button sits over the middle of the tile and stops its click, so it never opens the viewer; the rest of the tile still does. `RowCtx` gained a `scope` per list so the same tweet in two columns doesn't share a player. A GIF opened in the viewer decodes a second time there.

### 2026-10-24: Videos decoded with Media Foundation, in their own crate
- **Decision**: GIFs (and later videos) are decoded with Windows Media Foundation through the `windows` crate (0.62, the version GPUI already builds). An `IMFSourceReader` opens the MP4 URL itself (progressive download), converts to 32-bit RGB and hands frames to the app, which shows each one as a `RenderImage` in an `img`. The COM code lives in a new crate, `twitty-media`, whose own lint table denies `unsafe_code` and allows it only in the `mf` module (and its fixture generator), with a `// SAFETY:` comment per block.
- **Why**: GPUI's only video element (`surface`) is macOS-only, and its animated `img` only decodes GIF/WebP files, while Twitter's "GIFs" are H.264 MP4s. Media Foundation ships with Windows (HTTPS, AAC audio for later, and hardware decoding once a D3D device is handed to the reader; it decodes in software today) where ffmpeg would mean bundling DLLs and `openh264` would mean a separate demuxer. The workspace `forbid(unsafe_code)` can't be relaxed per module, so the unsafe code gets its own crate and `twitty`, `twitty-model` and `twitty-x` stay forbid-clean.
- **Consequences**: Playback is Windows-only (`Decoder::open` returns `Unsupported` elsewhere; the viewer keeps the poster). `mfplat.dll` and `mfreadwrite.dll` are loaded at run time with `LoadLibraryExW` (System32 only), not linked: windows-rs links with `raw-dylib`, so a linked import would stop `twitty.exe` and every test binary from starting on Windows N without the Media Feature Pack, or on Windows Server without its Media Foundation feature, which GitHub's runners don't install. There, playback fails soft with `Unavailable` and the decoder tests say they skip. Media Foundation is started once per process and never shut down: shutting it down after a failed download and starting it again crashed the process (a test now repeats failed downloads from a local 404 server). Every frame is a new atlas texture, so `ui::player` drops each frame from the atlas once it is off screen (and on release, through the window). The decoder tests run against a synthetic, committed `bars.mp4`.

### 2026-10-24: "For you" is never polled
- **Decision**: `poll::bucket` returns `None` for Home "For you", like conversations. It loads on open and on an explicit refresh only; "Following" still polls.
- **Why**: "For you" is algorithmic and re-ranked on every request, so polling it pulls in a churn of tweets that aren't "new" and moves the timeline under the reader.

### 2026-10-24: Random transaction id kept (M10) (superseded the same day, see "Real `x-client-transaction-id`")
- **Decision**: The planned port of Twitter's real `x-client-transaction-id` algorithm is not done.
- **Why**: Twitter still accepts random ids (verified with every live request this session), the algorithm depends on fast-changing web bundles, and a wrong port is worse than none. Enforcement is detected: error 353 becomes `TransactionIdRequired`, visible in the UI, the log and Diagnostics.
- **Consequences**: When 353 appears, follow the runbook in x-api.md.

### 2026-10-24: No TweetDeck deck sync (M10)
- **Decision**: Decks stay local (`decks.json`).
- **Why**: TweetDeck's deck storage uses undocumented endpoints we haven't observed, and syncing would write to the account's TweetDeck layout. Local decks have no such risk.

### 2026-10-24: Drift is measured per operation (M10)
- **Decision**: `XBackend` counts, per GraphQL operation, rows it couldn't translate and warnings, and keeps the last reason; Settings → Diagnostics shows them.
- **Why**: Translation fails soft, so a format change otherwise only shows as quietly missing rows.

### 2026-10-24: Polling may use a third of a feed's budget (M9)
- **Decision**: `state::poll` spaces polling so all columns of one feed type together use at most a third of its rate limit (min 30s, max 30 min). `XBackend` reports Twitter's observed limit and assumes 50 per 15 min before it knows.
- **Why**: Scrolling, refreshes and first loads need the rest; Twitter's limits differ a lot by operation (50 to 500 per 15 min), and several identical columns used to poll every 30s each.

### 2026-10-24: Shortcuts only in the `Deck` key context (M9)
- **Decision**: Single-letter shortcuts are bound in the `Deck` context; dialogs render outside that element.
- **Why**: Otherwise typing "like" in the compose box would like the selected tweet.

### 2026-10-24: Writes are tested against a mock server only (M8)
- **Decision**: Likes, retweets, bookmarks and tweets were implemented and tested with `wiremock`, but never sent to Twitter during development.
- **Why**: They change a real account and can't be fully undone (a tweet is seen, notifications are sent). The user makes the first real write.
- **Consequences**: The variable sets are from the web client, not from our own captures; the UI surfaces Twitter's error message if one is wrong.

### 2026-10-24: "Already done" is success (M8)
- **Decision**: Twitter error 139 (already liked) and 327 (already retweeted) don't roll back the optimistic state.
- **Why**: The account is already in the state the user asked for; a rollback would show the wrong state.

### 2026-10-24: Tweet rows navigate through the hub (M8)
- **Decision**: Row clicks emit `Navigate` on the shared `InteractionsHub` instead of calling the app.
- **Why**: Rows are rendered by free functions in columns and in the thread panel; the hub is the one entity they all share, and it keeps `post.rs` independent of the app's layout.

### 2026-10-24: No fixture from the user's own likes (M7)
- **Decision**: The `Likes` request was verified live on the signed-in account, but only public data (NASA's replies and media, a public list found through list search) became fixtures. `Likes` uses the same timeline shape, covered by those.
- **Why**: Even anonymised, a likes capture reveals what the account owner liked.

### 2026-10-24: One "Add column" dialog instead of per-type rail buttons (M7)
- **Decision**: The rail has a single **+**; the dialog parses links and handles (`state::add_column`), resolving handles to user ids before the column is created.
- **Why**: User, List and Tweet columns need input, and `FeedSource::User` needs the id (Twitter's user timelines take `userId`, not the handle).

### 2026-10-24: Sign-out only for sessions the app stored (M6)
- **Decision**: "Sign out" deletes the keyring entry. Sessions from env vars or session files show their source instead of a sign-out button.
- **Why**: The app can't remove those sources, so a sign-out would silently undo itself on the next launch.

### 2026-10-24: The UI builds backends through an injected `Connector` (M6)
- **Decision**: `main.rs` passes `AccountServices { connect, store }`; the sign-in dialog turns pasted cookies into a backend through `connect` and checks it with `viewer()` before `store.save`.
- **Why**: Keeps `twitty-x` out of the UI (architecture rule) and lets the whole flow be tested with `MockBackend` and `MemoryStore`, without touching the real Credential Manager in tests or CI.
- **Consequences**: Cookie validation lives in one place (`twitty_x::Session::new`); the form only normalises pasted text.

### 2026-10-24: No separate `PostStore` (M5)
- **Decision**: Rows stay owned by each `ColumnModel` (deduplicated by `FeedItem::key`). Viewer state that must agree across columns (liked, retweeted, bookmarked) lives in `InteractionsHub`, keyed by `PostId`. The planned shared `PostStore` isn't built.
- **Why**: The only thing columns must share today is viewer state, and the hub already does that. A shared store would add invalidation work without a user-visible gain.
- **Consequences**: Revisit if a feature needs one tweet's content to update everywhere (e.g. live metrics or edits).

### 2026-10-24: `XBackend` refuses writes until M8
- **Decision**: `XBackend::act` and `publish` return `BackendError::Upstream` without calling Twitter. The UI's optimistic like/repost/bookmark rolls back.
- **Why**: Wiring the real backend shouldn't silently turn existing buttons into writes on the user's account. Mutations get their own tests and review in M8.

### 2026-10-24: Columns hold off after rate limits and session errors
- **Decision**: `ColumnModel::begin` refuses any load until a `RateLimited` error's `reset_at`, and after `Unauthorized` refuses everything but an explicit `Refresh`. An empty page ends the feed even when Twitter sends a cursor.
- **Why**: With a real backend, the 30s poll and scroll-triggered paging would otherwise keep firing requests that can only fail, or page through empty results forever.

### 2026-10-24: Only live-verified variables are mapped
- **Decision**: `requests::feed` maps a `FeedSource` to an operation only when its variables were seen working live. The others return an error instead of a guess.
- **Why**: A wrong guess fails at Twitter with unhelpful errors, or worse, returns subtly different data.

### 2026-10-24: Twitter entity offsets are code points
- **Finding**: Across 453 entities in live captures (66 texts with emoji), 98 only match as Unicode code-point offsets and none only as UTF-16. The plan had assumed UTF-16.
- **Decision**: `translate::text` slices by code points, then emits byte ranges.

### 2026-10-24: GET→POST fallback before refreshing ids
- **Finding**: `SearchTimeline` answers 404 to GET even with a freshly discovered id, and works over POST.
- **Decision**: A GET 404 is retried once as POST and remembered per operation; only then are ids refreshed.

### 2026-10-24: Fixtures are anonymised live captures, snapshotted as text summaries
- **Decision**: Commit scrubbed full responses (`twitty-x-cli anonymise`, random key per run, leak check), and snapshot a compact line-per-row summary of the model output instead of serialising model types.
- **Why**: Real shapes catch drift that hand-written JSON misses. Summaries are readable in review and keep `twitty-model` free of test-only `Serialize` impls.

### 2026-10-24: Own toasts instead of GPUI Kit's notifications
- **Decision**: Action results show as toasts kept in `TwittyApp` (`state::toasts` for the queue, lifetimes and messages; `ui::toasts` draws them). The hub emits a `Notice` for each tweet action's outcome (it replaced the `ActionFailed` event, which was only logged); the profile sheet and **Copy tweet link** send theirs through the hub; the app adds "Tweet sent" itself. Time comes from `background_executor().now()`, so tests move it with `advance_clock`.
- **Why**: `window.push_notification` panics in a window opened without GPUI Kit's root (`add_window_view`, which every app test uses), and its cards have a fixed popover look with a coloured icon only, not a green or red card.
- **Consequences**: Follow failures show twice: the toast, and the reason under the profile header that was already there.

### 2026-10-24: Real `x-client-transaction-id` (retweet and bookmark were failing)
- **Finding**: With random transaction ids, `CreateRetweet` and `CreateBookmark` answered HTTP 404, and rediscovery found the same ids. The Relay `retweetButtonRetweetMutation` answered error 226 ("looks like it might be automated") on a real tweet. Likes, unbookmarking and reads still passed. With a real id, both legacy mutations reach the server again (error 144/34 for a nonexistent tweet), and a signed `CreateBookmark` on a real tweet succeeds. So the 404s were enforcement of this header, not dead ids. That the 226 had the same cause is likely but not yet confirmed by a real retweet; 226 can also be an account-level trust signal that no header fixes. An earlier commit the same day (`abe6456`) wrongly moved retweet to the Relay mutation; that is reverted.
- **Decision**: Port the web client's algorithm into `transport/transaction.rs` (after XClientTransaction, MIT), rather than depend on the `x-client-transaction` crate: it is about 250 lines, and owning it keeps the page parsing next to discovery's. The key material comes from the logged-in home page, cached for an hour. Any failure falls back to a random id with a warning, never to a failed request. `sha2` and `base64` were already in the build through other crates.
- **Consequences**: One extra page fetch per hour. The first request after launch waits for it (two fetches, each with a 20 s timeout), because concurrent requests queue on the signer's lock instead of each fetching the page. The algorithm depends on x.com's markup (`twitter-site-verification`, `loading-x-anim-*`, the `ondemand.s` chunk and its `(x[N],16)` indices), so the runbook in x-api.md lists them. Supersedes "Random transaction id kept (M10)".

### 2026-10-24: Twitter's logged-out site moved to a Relay client; discovery reads both formats
- **Finding**: `https://x.com/?lang=en` now loads `x-web` bundles (Vite + Relay). Operations are Relay persisted queries (`params:{id,name:"…Query",operationKind}`) without feature switches. No `responsive-web` bundle is referenced from the HTML or `sw.js`, so the community discovery methods find nothing there. The logged-in app still called `0toR0MxC06yY3Fc57GXBpA/HomeTimeline` (old-style path) on the same day.
- **Decision**: Discovery extracts both the legacy and the Relay formats, follows `assets/*.js` chunks, and runs logged in when a session exists. It is best effort: the catalog falls back to the cache and then the embedded ids, and a 404 triggers one refresh + one retry.
- **Settled live (same day)**: the logged-in app is the legacy client and uses `<id>/<OperationName>` with `features`/`fieldToggles`. Its operations are mostly in lazily loaded chunks listed in inline webpack chunk maps, which discovery now follows. Relay operations also answer on `<id>/<relayName>` (verified with `retweetButtonRetweetMutation`), but Twitty doesn't use them.

### 2026-10-24: Embedded fallback ids come from our own live discovery
- **Decision**: `data/operations.json` is generated from a logged-in `refresh-ids` run, including each operation's feature switches and field toggles. `data/default_features.json` (values for those switches) comes from `bun_x` (Apache-2.0).
- **Why**: 11 of the 18 ids `bun_x` had published the previous day were already stale. Regenerate the file after each live refresh that changes ids.

### 2026-10-24: Defer reactions to list scroll events
- **Decision**: The column's `ListState` scroll handler only captures the visible range and calls `cx.defer` to update the column.
- **Why**: GPUI invokes the handler while the `ListState` is mutably borrowed. Reading `logical_scroll_top()` inside it panicked on the first wheel scroll. The windowed tests in `ui/column.rs` reproduce this.

### 2026-10-24: Row identity for tombstones and unsupported entries
- **Decision**: `FeedItem::Unavailable` and `FeedItem::Unsupported` carry an `entry` string supplied by the backend (e.g. Twitter's timeline entry id). `FeedItem::key()` uses it.
- **Why**: Tombstones often have no tweet id. Keying on the reason would merge two different rows into one during de-duplication.

### 2026-10-24: App logic in `twitty::state`, views stay thin
- **Decision**: `twitty` is a library plus a binary. Paging, optimistic actions, decks and layout math are plain Rust in `state/`. GPUI views only render and forward input.
- **Why**: Plain unit tests are fast and deterministic. GPUI tests only cover the wiring.

### 2026-10-24: `Runtime::Inline` for tests
- **Decision**: The runtime bridge is an enum. Production uses a background tokio runtime. GPUI tests use `Inline`, which polls backend futures on the test executor.
- **Why**: GPUI's `run_until_parked()` can't wait for work on other threads, so tests on the tokio runtime would be flaky.

### 2026-10-24: Footer row in every column list
- **Decision**: `ListState` holds `items + 1` rows. The last one shows the loading spinner, the retry button or "You're all caught up".
- **Why**: Paging state stays visible without overlaying the list, and appending with `ListState::splice` naturally inserts before the footer.

### 2026-10-24: Own domain model with an anti-corruption layer
- **Decision**: All Twitter JSON is parsed in a private `twitty_x::wire` module and converted into `twitty-model` types by `twitty_x::translate`. Nothing outside `twitty-x` sees Twitter's structures.
- **Why**: Twitter's internal schema changes often. Keeping those changes in one module protects the UI, persisted data and tests from breaking.
- **Consequences**: Translation code has to be written. Snapshot tests are on model output, so Twitter drift appears as a `translate` diff rather than app-wide breakage.

### 2026-10-24: GPUI through GPUI Kit, pinned exactly
- **Decision**: Depend on `gpui-kit = "=0.8.0"` (`gpui-pre =0.3.9`) and commit `Cargo.lock`.
- **Why**: GPUI snapshots break the API. gpui-kit provides the 70+ components we need (avatar, popover, sheet, dialog, tabs, …) and a theme system.
- **Consequences**: Upgrades are deliberate, separate changes ([development.md](development.md)).

### 2026-10-24: Separate tokio runtime for networking
- **Decision**: `twitty-x` uses reqwest on a tokio runtime that runs on a background thread. The UI awaits its `JoinHandle`s from GPUI tasks.
- **Why**: GPUI's executor isn't tokio, and keeping `twitty-x` independent of GPUI makes it testable with plain `#[tokio::test]`.

### 2026-10-24: `list()`/`ListState` for timelines
- **Decision**: Use GPUI's `list` element (measured, variable height) for columns.
- **Why**: Tweet heights vary. `uniform_list` and `v_virtual_list` need heights up front.

### 2026-10-24: Login by pasting cookies (v1) (superseded, see "Sign in through a WebView2 window")
- **Decision**: The user pastes `auth_token` + `ct0`. They're stored in Windows Credential Manager.
- **Why**: `gpui-webview` doesn't support Windows, and building the Twitter login flow ourselves is fragile and risky.

### 2026-10-25: Sign in through a WebView2 window
- **Decision**: **Sign in on x.com** opens the real x.com login page in a WebView2 window (`wry` + `tao`), running in a child process (`twitty --login-helper`). The app reads `auth_token` and `ct0` from the window's cookie store and gets them back as one JSON line on the child's stdout. Pasting cookies stays as the fallback.
- **Why**:
  - Reimplementing Twitter's login flow (username/password through `onboarding/task.json`) means chasing `castle_token` fingerprints, Arkose captchas, 2FA variants and "unusual activity" checks. It also gets accounts locked, and it would have to handle the password.
  - The real page handles all of that, and Twitty never sees the password.
  - `wry` talks to WebView2 directly, so GPUI's lack of a Windows webview doesn't matter. It uses the same `windows` 0.62 as GPUI.
  - A child process keeps tao's event loop off GPUI's main thread.
  - The page runs InPrivate, so the only copy of the session is the keyring's.
- **Limits**:
  - Google and Apple sign-in don't work in embedded browsers.
  - The window itself is only checked by hand. The cookie-settling logic, the reply protocol and the dialog are unit- and view-tested with a fake window.

### 2026-10-24: Random `x-client-transaction-id` for now (superseded, see "Real `x-client-transaction-id`")
- **Decision**: Send a random 32-hex value per request, as `bun_x` does.
- **Why**: Twitter currently accepts it. The real algorithm is a known follow-up, triggered by error 353 ([x-api.md](x-api.md)).

### 2026-10-24: `bun_x` as reference, not as a dependency
- **Decision**: Write our own small `twitty-x` and borrow ideas and patterns from `bun_x` (Apache-2.0, with credit).
- **Why**: `bun_x` is ~14k lines and does far more than we need. We want to control the wire→model boundary.

### 2026-10-24: Exit animations through inert "leaving" copies
- **Decision**: Closing an overlay or the thread panel clears its state immediately and keeps the closed view in a `leaving` slot that only draws the exit animation, cleared by a timer task. The leaving layer takes no keys and no clicks: no handlers, no occluding backdrop, and a transparent occluding cover over its card.
- **Why**: GPUI has no unmount animation. Delaying the real close would make every "is it closed?" check (focus return, `has_dialog`, Esc order, tests) time-dependent; a live leaving copy would still accept clicks (e.g. compose's Tweet button, the viewer's ‹ › stepping columns once it's gone).
- **Consequences**: For ~120ms after closing, a click on the fading card itself is swallowed; clicks beside it work. Rows animate only while a `motion::Recent` entry is live, because `list()` drops element state for rows scrolled out. The "Animations" setting maps to GPUI's `reduce_motion`.

### 2026-10-24: Image viewer caps images at their decoded size
- **Decision**: The viewer gives `img` only a max width/height (from the window size) and no explicit size, so GPUI lays it out at the decoded bitmap's size, capped by the available box.
- **Why**: `ImageRef.size` is Twitter's `original_info`, but the URL served is a smaller variant (`?name=medium`, ≤ 1200 px). Capping at `ImageRef.size` would still upscale big originals, and computing the variant's size in the UI would leak Twitter knowledge out of `twitty-x`.
- **Consequences**: Bitmap pixels map to logical pixels, so at 150% display scaling an image can still be up to 1.5× physically. Showing large originals sharply on big screens needs a full-size variant in the model (a separate change).

### 2026-10-24: Remote images are shrunk on the CPU before GPUI draws them
- **Decision**: Media tiles, quote and link-card images and profile gallery tiles load through `ui::remote_image` (a GPUI `Asset` that fetches, decodes and resamples with the `image` crate) instead of `img(url)`. The target is the drawn box in device pixels, rounded up in 1.25× buckets; images already within 1.25× of it are left as they are.
- **Why**: The Windows renderer uploads textures with one mip level and samples them bilinearly, so drawing a `?name=medium` (≤ 1200 px) photo at a third of its size skips most source pixels and looks pixelated. GPUI is pinned, so the fix can't go in the renderer. Requesting a smaller Twitter variant would still leave 2–3× shrinks at 100% scaling and be too soft at 200%.
- **Consequences**: GPUI's shrink is capped at about 1.5× (bucket plus threshold), which bilinear sampling handles cleanly. Each image is fetched again for each new bucket (a column resized or the window moved to a different scale factor); GPUI's asset cache keeps every loaded copy for the session, as it already did for `img(url)`. Only the first frame is decoded, which is fine for Twitter's still previews. Uses `image` 0.25 (the version GPUI uses, needed for `RenderImage`'s `Frame`) and `futures` (to read the response body); both were already in the build.

### 2026-10-24: Profiles open in the side sheet, not as a column
- **Decision**: Clicking an avatar or a name (tweet author, retweeter, quoted author, notification actor) opens a profile sheet with Tweets / Media tabs. Adding the account as a column is an explicit action: **Open column** on the hover card, or **+** in the sheet. The profile sheet and the thread panel share one side-sheet slot (`Sheet` in `ui/app.rs`); opening one replaces the other, with no back stack.
- **Why**: A click to look at someone shouldn't change the saved deck. One slot keeps the backdrop, slide-in/out, the inert leaving copy and the Esc order identical for both sheets.
- **Consequences**: A tweet opened from a profile replaces the profile; going back means clicking the author again. A back stack can be added in `TwittyApp::show_sheet` if that becomes annoying.

### 2026-10-24: Shared tweet links point at vxtwitter
- **Decision**: Clicking a tweet's time and **Copy tweet link** use the tweet's canonical `Post.url` with its host swapped for `vxtwitter.com` (`state::links::vx_link`). (The action bar's share button, which opened the x.com link, was removed on 2026-10-24.)
- **Why**: vxtwitter links embed properly (video, full text) when pasted into Discord and other chat apps. Swapping the host of `Post.url` keeps the UI from building Twitter URLs out of opaque IDs.
- **Consequences**: The links depend on a third-party mirror being up. If it goes away, change `VX_HOST` in one place.

### 2026-10-24: Column settings dialog; widths in pixels
- **Decision**: Each column has a settings dialog (name, width slider, sound with volume, system notification) instead of the width-cycling header button. Widths are stored as pixels (280–640), so the deck file went to `schema_version` 2, which maps the v1 presets to the pixels they stood for. The header's buttons only show on hover.
- **Why**: Three presets were too coarse for wide screens, and one dialog has room for per-column options. Hiding the buttons gives the title the whole header when the pointer is elsewhere.
- **Consequences**: Out-of-range widths and volumes in a hand-edited file are clamped on load. Settings apply live; only slider drags wait for the release before saving.

### 2026-10-24: New-post sound through rodio, notifications through GPUI
- **Decision**: The sound is an embedded mp3 played with `rodio` (`playback` + `mp3` features only), one short-lived thread per play. System notifications use GPUI's own `App::show_system_notification`, not `notify-rust`. `main.rs` calls `cx.set_app_identity("Twitty.Twitty", "Twitty")` before anything else.
- **Why**: GPUI has no audio. On Windows GPUI shows toasts itself, registers the AppUserModelID for the current user (so toasts appear without a Start-menu shortcut) and makes them silent, so the column's own sound is the only one. GPUI's test platform records notifications, so they're tested without a fake.
- **Consequences**: Without an audio device the sound is skipped with a warning. The app's taskbar identity is now `Twitty.Twitty`, so an existing taskbar pin may need re-pinning once. Toasts don't act on a click yet (GPUI's `on_system_notification_response` could open the column).

### 2026-10-24: Own title bar instead of GPUI Kit's `TitleBar`
- **Decision**: `ui::title_bar` is a copy of GPUI Kit's `TitleBar` (drag area, Windows caption buttons, Linux/macOS move and double-click) with a 39px bar and 54px-wide window buttons. `main.rs` still uses `TitleBar::window_options()`.
- **Why**: GPUI Kit's height (34px) and button width (34px) are private constants, so they can't be styled.
- **Consequences**: Fixes to GPUI Kit's title bar don't reach ours; compare with `gpui-component`'s `title_bar.rs` when upgrading GPUI Kit.

### 2026-10-24: People search through `SearchTimeline`, debounced
- **Decision**: The rail's **Search people** dialog uses `SearchTimeline` with `product: "People"`, not the v1.1 `search/typeahead.json` endpoint. It has no search button: the search follows the input once typing pauses for 300ms.
- **Why**: The People tab returns full GraphQL user objects (counts, banner, relationship), the same shape as `UserByScreenName`, so the existing profile translation applies and a result opens straight in the profile sheet without a second lookup. Typeahead returns v1.1 user objects that would need their own translator.
- **Consequences**: Each settled query spends the `SearchTimeline` budget (about 50 per 15 minutes) shared with search columns. The debounce and skipping a repeated query keep that to about one request per word typed, but long typing sessions can still run the budget down. Only the first page (20 accounts) is shown.

### 2026-10-24: Decks in the sidebar, columns no longer listed there
- **Decision**: The sidebar lists decks (each with a user-picked icon) instead of one button per column, and the deck tab bar above the columns is gone. Order: Tweet, Search people, Add column, separator, decks, New deck. The sidebar can be expanded (labels + a pencil per deck); "New deck" and the pencil open the same dialog (`ui/deck_settings.rs`), which only makes a new deck on **Create**. Removing a deck moved from the tab's × into that dialog.
- **Why**: Switching decks is the navigation that needs a permanent place; the column buttons duplicated the deck itself (h/l and the arrows still move between columns).
- **Consequences**: `Deck` gained `icon: DeckIcon`, stored by name with `#[serde(default)]` and no version bump (per the rule in `domain-model.md`); unknown names load as the default icon, so a file from a newer build still loads. `settings.json` gained `sidebar_expanded`.

### 2026-10-24: The list picker reads `ListsManagementPageTimeline`
- **Decision**: The **Add column** dialog offers the viewer's lists in a select, loaded with `ListsManagementPageTimeline` (the x.com "Lists" page), keeping only its "Your Lists" module. The link/id field stays below for lists of other accounts.
- **Why**: That page returns owned *and* followed lists in one request without needing the viewer's user id. `ListOwnerships` takes a `userId` and returns only the lists that account owns, not the ones it follows.
- **Consequences**: The suggestions on the same page ("Discover new Lists") are told apart by their entry id prefix (`list-to-follow-module` vs `owned-subscribed-list-module`); if the "Your Lists" one goes missing, `viewer_lists` records a health warning (an empty account still has the module, holding a prompt), so the debug panel and the live test notice a rename. Only the first 100 lists are offered.

### 2026-10-24: Long tweets fold by character count
- **Decision**: Outside the thread panel, a tweet whose shown text (translation included) is over 320 characters is cut after about 280 and gets a **Show more** link (`state::long_text`). The model has no "long tweet" flag; the cut is decided from the text alone.
- **Why**: Twitter Blue's note tweets can run to thousands of characters and fill a whole column. Cutting the `RichText` (rather than clamping lines in the view) keeps the decision in plain, tested Rust and lets the cut avoid splitting a link or mention. The 40-character margin over 280 keeps classic tweets, whose links display longer than Twitter counts them, from ever being folded.
- **Consequences**: A classic tweet in a script Twitter counts double (CJK) is at most 140 characters, so it never folds; a long tweet with many short lines can still be tall when folded. The choice to unfold lasts for the session and is shared by every column.

### 2026-10-24: CI caches survive failures and timeouts
- **Decision**: Both CI jobs use `rust-cache` with `cache-on-failure: true` and `save-if` on `main`. They give the compile steps step-level timeouts below the job timeout, build without debug info, disable Defender real-time scanning, and only cancel in-progress runs for pull requests.
- **Why**: `rust-cache` only saved after a successful job. A cold build of the whole GPUI tree ran into the 60-minute timeout and saved nothing, so every run started cold again, and pushes to `main` cancelled the run before it could save. A step timeout fails the step normally so the save step still runs, which a job timeout doesn't guarantee. Without debug info, `target/` and the cache tarballs are much smaller, which makes them faster to save and restore and keeps both caches under the 10 GB repository quota.
- **Consequences**: The debug and release dependency trees are still compiled separately (they share almost nothing), but each one only after `Cargo.lock` or the toolchain changes. Backtraces in failing CI tests have no line numbers. `rust-lld` as the linker could speed things up further but hasn't been tried with `embed-resource`'s `.res` file.

### 2026-10-24: Emoji drawn with a bundled, patched Twemoji font
- **Decision**: Twitty bundles Mozilla's COLR build of Twemoji (v0.7.0) and draws emoji with it by giving their byte ranges the `Twemoji Mozilla` family (`state::emoji` finds them, `ui::emoji` builds the `StyledText`). Each colour glyph in the bundled font gets an outline made of its own layers (`resources/fonts/fill_base_glyphs.py`).
- **Why**: Twitter's emoji look different from Segoe UI Emoji, and a Twitter client should show the ones people chose. A font keeps emoji inside GPUI's text layout (wrapping, truncation, hit-testing for links), which inline images would break. Three GPUI limits shaped how it's done:
  - `FontFallbacks` on Windows only looks for installed fonts, so the bundled font can't be a fallback. Installing it for the user would change their system.
  - `HighlightStyle` has no font family. `with_font_family_overrides` only applies to runs that lie entirely inside an override, so emoji ranges are also passed as highlights to cut the runs.
  - GPUI sizes a glyph from its base outline before painting the COLR layers. Upstream Twemoji Mozilla leaves base glyphs empty, so without the patch every emoji is invisible.
- **Consequences**: About 1.7 MB more in the binary. Emoji newer than Unicode 14 fall back to Segoe UI Emoji, and new ZWJ sequences whose parts exist in Twemoji (🐦‍🔥) show as their separate parts. Keycaps stay with Segoe, because GPUI doesn't apply the font's keycap ligature. Only Windows has been checked. The compose box and other gpui-component `Input`s can't style ranges, so they keep Segoe UI Emoji. The artwork is CC-BY 4.0: the attribution lives in `resources/fonts/README.md`.
