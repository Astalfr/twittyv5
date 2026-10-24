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

`twitty` uses `twitty-x` only in `main.rs`, to build an `XBackend` and hand it over as `Arc<dyn Backend>`.

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

1. `twitty::runtime` starts a multi-threaded **tokio runtime on a background thread** at startup.
2. A view calls `runtime.spawn(backend.feed(source, page))`, which returns a `tokio::task::JoinHandle`.
3. The view awaits that handle inside `cx.spawn(async move |this, cx| …)`. A `JoinHandle` can be awaited from any executor.
4. When the result arrives, the task updates the entity through its `WeakEntity` and calls `cx.notify()`.

Images use GPUI's own loader, with `cx.set_http_client(gpui-pre-reqwest-client)` set at startup.

## Data flow example: opening the Home column

1. `DeckView` creates a `ColumnView` for `FeedSource::Home { mode: Following }`.
2. `ColumnView` asks the backend for `PageRequest::First` through the runtime bridge, and shows skeleton rows meanwhile.
3. `XBackend::feed` resolves the `HomeLatestTimeline` queryId, sends the request and translates the response into a `FeedPage`.
4. `ColumnView` adds the items to the shared `PostStore` (deduplicated by `PostId`) and keeps its own ordered list of IDs. It then calls `ListState::splice` and `cx.notify()`.
5. Scrolling near the bottom triggers `PageRequest::Older(token)`. The "new posts" poll uses `PageRequest::Newer(token)`.

## App state (`twitty` crate, planned)

- `AppState` (global entity) holds the session status, `Arc<dyn Backend>`, `PostStore`, `DeckStore` and settings.
- `PostStore` holds the single shared copy of each post and profile. Updates from actions (like/repost) are applied there, so every column shows the change.
- `DeckStore` holds the decks and columns (`FeedSource` + width + options). They're saved as JSON in `%APPDATA%\Twitty\decks.json`, with a `schema_version`.
- Credentials live in Windows Credential Manager (`keyring`), never in plain files.
