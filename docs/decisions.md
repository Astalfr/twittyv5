# Decision log

Lightweight ADRs: newest first. Each entry: date, decision, why, consequences. Add one whenever a choice isn't obvious from the code, or when X forces a change.

---

### 2026-10-24: X's logged-out site moved to a Relay client; discovery reads both formats
- **Finding**: `https://x.com/?lang=en` now loads `x-web` bundles (Vite + Relay). Operations are Relay persisted queries (`params:{id,name:"…Query",operationKind}`) without feature switches. No `responsive-web` bundle is referenced from the HTML or `sw.js`, so the community discovery methods find nothing there. The logged-in app still called `0toR0MxC06yY3Fc57GXBpA/HomeTimeline` (old-style path) on the same day.
- **Decision**: Discovery extracts both the legacy and the Relay formats, follows `assets/*.js` chunks, and runs logged in when a session exists. It is best effort: the catalog falls back to the cache and then the embedded ids, and a 404 triggers one refresh + one retry.
- **Open**: how the logged-in client names Relay operations in URLs, and whether Relay calls send `features`. To be settled by the first logged-in capture.

### 2026-10-24: `bun_x` ids and feature defaults embedded as fallback
- **Decision**: `data/operations.json` holds the observed `HomeTimeline` id plus ids from `bun_x`'s catalog (marked with their source). `data/default_features.json` comes from `bun_x` (Apache-2.0).
- **Why**: A working starting point without a live session. Every entry records where it came from, so stale ones are easy to spot.

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
