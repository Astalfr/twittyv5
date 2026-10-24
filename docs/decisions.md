# Decision log

Lightweight ADRs: newest first. Each entry: date, decision, why, consequences. Add one whenever a choice isn't obvious from the code, or when X forces a change.

---

### 2026-10-24: "For you" is never polled
- **Decision**: `poll::bucket` returns `None` for Home "For you", like conversations. It loads on open and on an explicit refresh only; "Following" still polls.
- **Why**: "For you" is algorithmic and re-ranked on every request, so polling it pulls in a churn of posts that aren't "new" and moves the timeline under the reader.

### 2026-10-24: Random transaction id kept (M10)
- **Decision**: The planned port of X's real `x-client-transaction-id` algorithm is not done.
- **Why**: X still accepts random ids (verified with every live request this session), the algorithm depends on fast-changing web bundles, and a wrong port is worse than none. Enforcement is detected: error 353 becomes `TransactionIdRequired`, visible in the UI, the log and Diagnostics.
- **Consequences**: When 353 appears, follow the runbook in x-api.md.

### 2026-10-24: No X Pro deck sync (M10)
- **Decision**: Decks stay local (`decks.json`).
- **Why**: X Pro's deck storage uses undocumented endpoints we haven't observed, and syncing would write to the account's X Pro layout. Local decks have no such risk.

### 2026-10-24: Drift is measured per operation (M10)
- **Decision**: `XBackend` counts, per GraphQL operation, rows it couldn't translate and warnings, and keeps the last reason; Settings → Diagnostics shows them.
- **Why**: Translation fails soft, so a format change otherwise only shows as quietly missing rows.

### 2026-10-24: Polling may use a third of a feed's budget (M9)
- **Decision**: `state::poll` spaces polling so all columns of one feed type together use at most a third of its rate limit (min 30s, max 30 min). `XBackend` reports X's observed limit and assumes 50 per 15 min before it knows.
- **Why**: Scrolling, refreshes and first loads need the rest; X's limits differ a lot by operation (50 to 500 per 15 min), and several identical columns used to poll every 30s each.

### 2026-10-24: Shortcuts only in the `Deck` key context (M9)
- **Decision**: Single-letter shortcuts are bound in the `Deck` context; dialogs render outside that element.
- **Why**: Otherwise typing "like" in the compose box would like the selected post.

### 2026-10-24: Writes are tested against a mock server only (M8)
- **Decision**: Likes, reposts, bookmarks and posts were implemented and tested with `wiremock`, but never sent to X during development.
- **Why**: They change a real account and can't be fully undone (a post is seen, notifications are sent). The user makes the first real write.
- **Consequences**: The variable sets are from the web client, not from our own captures; the UI surfaces X's error message if one is wrong.

### 2026-10-24: "Already done" is success (M8)
- **Decision**: X error 139 (already liked) and 327 (already reposted) don't roll back the optimistic state.
- **Why**: The account is already in the state the user asked for; a rollback would show the wrong state.

### 2026-10-24: Post rows navigate through the hub (M8)
- **Decision**: Row clicks emit `Navigate` on the shared `InteractionsHub` instead of calling the app.
- **Why**: Rows are rendered by free functions in columns and in the thread panel; the hub is the one entity they all share, and it keeps `post.rs` independent of the app's layout.

### 2026-10-24: No fixture from the user's own likes (M7)
- **Decision**: The `Likes` request was verified live on the signed-in account, but only public data (NASA's replies and media, a public list found through list search) became fixtures. `Likes` uses the same timeline shape, covered by those.
- **Why**: Even anonymised, a likes capture reveals what the account owner liked.

### 2026-10-24: One "Add column" dialog instead of per-type rail buttons (M7)
- **Decision**: The rail has a single **+**; the dialog parses links and handles (`state::add_column`), resolving handles to user ids before the column is created.
- **Why**: User, List and Post columns need input, and `FeedSource::User` needs the id (X's user timelines take `userId`, not the handle).

### 2026-10-24: Sign-out only for sessions the app stored (M6)
- **Decision**: "Sign out" deletes the keyring entry. Sessions from env vars or session files show their source instead of a sign-out button.
- **Why**: The app can't remove those sources, so a sign-out would silently undo itself on the next launch.

### 2026-10-24: The UI builds backends through an injected `Connector` (M6)
- **Decision**: `main.rs` passes `AccountServices { connect, store }`; the sign-in dialog turns pasted cookies into a backend through `connect` and checks it with `viewer()` before `store.save`.
- **Why**: Keeps `twitty-x` out of the UI (architecture rule) and lets the whole flow be tested with `MockBackend` and `MemoryStore`, without touching the real Credential Manager in tests or CI.
- **Consequences**: Cookie validation lives in one place (`twitty_x::Session::new`); the form only normalises pasted text.

### 2026-10-24: No separate `PostStore` (M5)
- **Decision**: Rows stay owned by each `ColumnModel` (deduplicated by `FeedItem::key`). Viewer state that must agree across columns (liked, reposted, bookmarked) lives in `InteractionsHub`, keyed by `PostId`. The planned shared `PostStore` isn't built.
- **Why**: The only thing columns must share today is viewer state, and the hub already does that. A shared store would add invalidation work without a user-visible gain.
- **Consequences**: Revisit if a feature needs one post's content to update everywhere (e.g. live metrics or edits).

### 2026-10-24: `XBackend` refuses writes until M8
- **Decision**: `XBackend::act` and `publish` return `BackendError::Upstream` without calling X. The UI's optimistic like/repost/bookmark rolls back.
- **Why**: Wiring the real backend shouldn't silently turn existing buttons into writes on the user's account. Mutations get their own tests and review in M8.

### 2026-10-24: Columns hold off after rate limits and session errors
- **Decision**: `ColumnModel::begin` refuses any load until a `RateLimited` error's `reset_at`, and after `Unauthorized` refuses everything but an explicit `Refresh`. An empty page ends the feed even when X sends a cursor.
- **Why**: With a real backend, the 30s poll and scroll-triggered paging would otherwise keep firing requests that can only fail, or page through empty results forever.

### 2026-10-24: Only live-verified variables are mapped
- **Decision**: `requests::feed` maps a `FeedSource` to an operation only when its variables were seen working live. The others return an error instead of a guess.
- **Why**: A wrong guess fails at X with unhelpful errors, or worse, returns subtly different data.

### 2026-10-24: X entity offsets are code points
- **Finding**: Across 453 entities in live captures (66 texts with emoji), 98 only match as Unicode code-point offsets and none only as UTF-16. The plan had assumed UTF-16.
- **Decision**: `translate::text` slices by code points, then emits byte ranges.

### 2026-10-24: GET→POST fallback before refreshing ids
- **Finding**: `SearchTimeline` answers 404 to GET even with a freshly discovered id, and works over POST.
- **Decision**: A GET 404 is retried once as POST and remembered per operation; only then are ids refreshed.

### 2026-10-24: Fixtures are anonymised live captures, snapshotted as text summaries
- **Decision**: Commit scrubbed full responses (`twitty-x-cli anonymise`, random key per run, leak check), and snapshot a compact line-per-row summary of the model output instead of serialising model types.
- **Why**: Real shapes catch drift that hand-written JSON misses. Summaries are readable in review and keep `twitty-model` free of test-only `Serialize` impls.

### 2026-10-24: X's logged-out site moved to a Relay client; discovery reads both formats
- **Finding**: `https://x.com/?lang=en` now loads `x-web` bundles (Vite + Relay). Operations are Relay persisted queries (`params:{id,name:"…Query",operationKind}`) without feature switches. No `responsive-web` bundle is referenced from the HTML or `sw.js`, so the community discovery methods find nothing there. The logged-in app still called `0toR0MxC06yY3Fc57GXBpA/HomeTimeline` (old-style path) on the same day.
- **Decision**: Discovery extracts both the legacy and the Relay formats, follows `assets/*.js` chunks, and runs logged in when a session exists. It is best effort: the catalog falls back to the cache and then the embedded ids, and a 404 triggers one refresh + one retry.
- **Settled live (same day)**: the logged-in app is the legacy client and uses `<id>/<OperationName>` with `features`/`fieldToggles`. Its operations are mostly in lazily loaded chunks listed in inline webpack chunk maps, which discovery now follows. Still unknown (not needed): URL naming of Relay operations.

### 2026-10-24: Embedded fallback ids come from our own live discovery
- **Decision**: `data/operations.json` is generated from a logged-in `refresh-ids` run, including each operation's feature switches and field toggles. `data/default_features.json` (values for those switches) comes from `bun_x` (Apache-2.0).
- **Why**: 11 of the 18 ids `bun_x` had published the previous day were already stale. Regenerate the file after each live refresh that changes ids.

### 2026-10-24: Defer reactions to list scroll events
- **Decision**: The column's `ListState` scroll handler only captures the visible range and calls `cx.defer` to update the column.
- **Why**: GPUI invokes the handler while the `ListState` is mutably borrowed. Reading `logical_scroll_top()` inside it panicked on the first wheel scroll. The windowed tests in `ui/column.rs` reproduce this.

### 2026-10-24: Row identity for tombstones and unsupported entries
- **Decision**: `FeedItem::Unavailable` and `FeedItem::Unsupported` carry an `entry` string supplied by the backend (e.g. X's timeline entry id). `FeedItem::key()` uses it.
- **Why**: Tombstones often have no post id. Keying on the reason would merge two different rows into one during de-duplication.

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
- **Decision**: All X JSON is parsed in a private `twitty_x::wire` module and converted into `twitty-model` types by `twitty_x::translate`. Nothing outside `twitty-x` sees X's structures.
- **Why**: X's internal schema changes often. Keeping those changes in one module protects the UI, persisted data and tests from breaking.
- **Consequences**: Translation code has to be written. Snapshot tests are on model output, so X drift appears as a `translate` diff rather than app-wide breakage.

### 2026-10-24: GPUI through GPUI Kit, pinned exactly
- **Decision**: Depend on `gpui-kit = "=0.8.0"` (`gpui-pre =0.3.9`) and commit `Cargo.lock`.
- **Why**: GPUI snapshots break the API. gpui-kit provides the 70+ components we need (avatar, popover, sheet, dialog, tabs, …) and a theme system.
- **Consequences**: Upgrades are deliberate, separate changes ([development.md](development.md)).

### 2026-10-24: Separate tokio runtime for networking
- **Decision**: `twitty-x` uses reqwest on a tokio runtime that runs on a background thread. The UI awaits its `JoinHandle`s from GPUI tasks.
- **Why**: GPUI's executor isn't tokio, and keeping `twitty-x` independent of GPUI makes it testable with plain `#[tokio::test]`.

### 2026-10-24: `list()`/`ListState` for timelines
- **Decision**: Use GPUI's `list` element (measured, variable height) for columns.
- **Why**: Post heights vary. `uniform_list` and `v_virtual_list` need heights up front.

### 2026-10-24: Login by pasting cookies (v1)
- **Decision**: The user pastes `auth_token` + `ct0`. They're stored in Windows Credential Manager.
- **Why**: `gpui-webview` doesn't support Windows, and building the X login flow ourselves is fragile and risky.

### 2026-10-24: Random `x-client-transaction-id` for now
- **Decision**: Send a random 32-hex value per request, as `bun_x` does.
- **Why**: X currently accepts it. The real algorithm is a known follow-up, triggered by error 353 ([x-api.md](x-api.md)).

### 2026-10-24: `bun_x` as reference, not as a dependency
- **Decision**: Write our own small `twitty-x` and borrow ideas and patterns from `bun_x` (Apache-2.0, with credit).
- **Why**: `bun_x` is ~14k lines and does far more than we need. We want to control the wire→model boundary.

### 2026-10-24: Exit animations through inert "leaving" copies
- **Decision**: Closing an overlay or the thread panel clears its state immediately and keeps the closed view in a `leaving` slot that only draws the exit animation, cleared by a timer task. The leaving layer takes no keys and no clicks: no handlers, no occluding backdrop, and a transparent occluding cover over its card.
- **Why**: GPUI has no unmount animation. Delaying the real close would make every "is it closed?" check (focus return, `has_dialog`, Esc order, tests) time-dependent; a live leaving copy would still accept clicks (e.g. compose's Post button, the viewer's ‹ › stepping columns once it's gone).
- **Consequences**: For ~120ms after closing, a click on the fading card itself is swallowed; clicks beside it work. Rows animate only while a `motion::Recent` entry is live, because `list()` drops element state for rows scrolled out. The "Animations" setting maps to GPUI's `reduce_motion`.

### 2026-10-24: Image viewer caps images at their decoded size
- **Decision**: The viewer gives `img` only a max width/height (from the window size) and no explicit size, so GPUI lays it out at the decoded bitmap's size, capped by the available box.
- **Why**: `ImageRef.size` is X's `original_info`, but the URL served is a smaller variant (`?name=medium`, ≤ 1200 px). Capping at `ImageRef.size` would still upscale big originals, and computing the variant's size in the UI would leak X knowledge out of `twitty-x`.
- **Consequences**: Bitmap pixels map to logical pixels, so at 150% display scaling an image can still be up to 1.5× physically. Showing large originals sharply on big screens needs a full-size variant in the model (a separate change).

### 2026-10-24: Profiles open in the side sheet, not as a column
- **Decision**: Clicking an avatar or a name (post author, reposter, quoted author, notification actor) opens a profile sheet with Tweets / Media tabs. Adding the account as a column is an explicit action: **Open column** on the hover card, or **+** in the sheet. The profile sheet and the thread panel share one side-sheet slot (`Sheet` in `ui/app.rs`); opening one replaces the other, with no back stack.
- **Why**: A click to look at someone shouldn't change the saved deck. One slot keeps the backdrop, slide-in/out, the inert leaving copy and the Esc order identical for both sheets.
- **Consequences**: A post opened from a profile replaces the profile; going back means clicking the author again. A back stack can be added in `TwittyApp::show_sheet` if that becomes annoying.

### 2026-10-24: Shared post links point at vxtwitter
- **Decision**: Clicking a post's time and **Copy tweet link** use the post's canonical `Post.url` with its host swapped for `vxtwitter.com` (`state::links::vx_link`). The share button in the action bar still opens the canonical x.com link.
- **Why**: vxtwitter links embed properly (video, full text) when pasted into Discord and other chat apps. Swapping the host of `Post.url` keeps the UI from building X URLs out of opaque IDs.
- **Consequences**: The links depend on a third-party mirror being up. If it goes away, change `VX_HOST` in one place.
