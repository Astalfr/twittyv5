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
```

| Crate | Responsibility | May depend on | Must not depend on |
|---|---|---|---|
| `twitty-model` | Stable domain types (`Post`, `Profile`, `FeedItem`, …), `Backend` trait, `BackendError`, `MockBackend` (feature `mock`) | serde, chrono, url, smol_str, thiserror | gpui, reqwest, tokio, twitty-x |
| `twitty-x` | Everything X-specific: HTTP, auth, query-ID discovery, GraphQL ops, wire parsing, translation into the model | twitty-model, reqwest, tokio, serde_json, regex | gpui, twitty |
| `twitty` | UI, app state, persistence of decks and settings, keyring, runtime bridge | twitty-model, twitty-x, gpui-kit | (X JSON types, which aren't public anyway) |

`twitty` uses `twitty-x` only in `main.rs`, to build an `XBackend` and hand it over as `Arc<dyn Backend>`. Until M5, `main.rs` uses `MockBackend` and shows a "Demo mode" banner.

`twitty` is a library plus a thin binary:

| Module | Contents |
|---|---|
| `state::column` | `ColumnModel`: paging, cursors, de-duplication, unseen count, `ListChange` instructions for the view |
| `state::interactions` | Optimistic like/repost/bookmark overrides with rollback |
| `state::deck` | `DeckFile` (versioned `decks.json`), column add/remove/move/width |
| `state::media_layout` | Geometry of the 1–4 media grid |
| `format` | Relative times and compact counters |
| `runtime` | `Runtime::Tokio` (production) / `Runtime::Inline` (tests) |
| `ui::app` | Root view: rail + horizontally scrolling deck, persists deck changes |
| `ui::column` | Column view: header, `list()` + `ListState`, footer row, polling, "new posts" pill |
| `ui::post` | Row rendering: posts, reposts, threads, media, quotes, link cards, polls, action bar |
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
- **discovery**: resolves `operationName → (queryId, features)`. The order is runtime cache → refresh from X's JS bundles → embedded fallback.
- **ops**: one module per GraphQL operation. Each builds `variables`/`features`/`fieldToggles` and returns raw JSON.
- **wire**: walks X's JSON (instructions, entries, results). It tolerates missing fields and unknown typenames.
- **translate**: the only code that knows both X's shapes and the model. All normalisation happens here.

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
3. `XBackend::feed` resolves the `HomeLatestTimeline` queryId, sends the request and translates the response into a `FeedPage`.
4. `ColumnView` adds the items to the shared `PostStore` (deduplicated by `PostId`) and keeps its own ordered list of IDs. It then calls `ListState::splice` and `cx.notify()`.
5. Scrolling near the bottom triggers `PageRequest::Older(token)`. The "new posts" poll uses `PageRequest::Newer(token)`.

## App state (`twitty` crate)

- `TwittyApp` (root view) owns the `DeckFile`, the column views and the shared `InteractionsHub`.
- Each `ColumnView` owns its `ColumnModel` (rows + cursors) and its `ListState`. The list has one extra footer row (spinner, retry, "caught up").
- The viewer's likes, reposts and bookmarks are overrides in `InteractionsHub`, so every column shows the change instantly. Failures roll back.
- Decks are saved as JSON in `%APPDATA%\Twitty\config\decks.json` (atomic write), with a `schema_version`.
- Credentials live in Windows Credential Manager (`keyring`), never in plain files.
