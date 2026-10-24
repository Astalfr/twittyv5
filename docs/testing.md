# Testing

**Goal: X can change under us and we can refactor freely without breaking features.** Every feature is covered by an automated test on the layer where it lives, and a change isn't finished until all checks pass.

## Definition of done

1. Tests for the new or changed behaviour are added in the same change. A bug fix starts with a failing test.
2. `cargo fmt --all`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace`
5. Snapshot changes are reviewed with `cargo insta review` and explained in the commit message.
6. The matching `docs/` page is updated.

Never `#[ignore]`, weaken or delete a test to get a green run.

## Test layers

| Layer | Where | Tool | What it proves |
|---|---|---|---|
| Model unit tests | `twitty-model/src/**` `#[cfg(test)]` | std, `proptest` | `RichText` invariants, ID newtypes, serde round-trips |
| Translation snapshots | `twitty-x/tests/translate.rs` + `tests/fixtures/*.json` | `insta` (text summaries) | 9 anonymised live responses translate with **zero** `Unsupported` rows and zero warnings · compact per-row summaries are snapshotted · fixtures together must cover reposts, threads, notifications, quotes, link cards, video, GIF, multi-image, replies, long posts and mention/hashtag/link spans · promoted posts skipped · authors shared per page |
| Wire robustness | `twitty-x/tests/wire_robustness.rs` | `proptest`, mutated fixtures | Missing or renamed fields and unknown typenames never panic, and become `Unsupported` |
| Entity offsets | `twitty-x/src/translate/text.rs` tests | `proptest` | Code-point → byte conversion with emoji and CJK, display ranges, media-link removal, HTML decoding. A property test feeds random entities and ranges: never panics, spans always valid. |
| Backend | `twitty-x/tests/backend.rs` | `wiremock`, committed fixtures as bodies | Each `FeedSource` calls its operation and translates with 0 `Unsupported` · older/newer cursors sent · search over POST with `product` · 401 → `Unauthorized` · unmapped sources fail without a request · thread split around the focal post, missing focal → `NotFound` · `Viewer`/`UserByScreenName` → `Profile` · writes refused without a request |
| Transport | `twitty-x/tests/transport.rs` | `wiremock`, `tokio::test` | Required headers present · `x-csrf-token == ct0` · fresh txid per request · cookie rotation used next + hook · 429 reset + local gate per operation · 5xx retried, 401 not · 404 → discovery → retry once with discovered features · override skips discovery · POST body for mutations · cursor forwarded · secrets never in `Debug`/errors |
| Discovery + catalog | `twitty-x/src/{discovery,catalog}.rs` unit tests | tiny synthetic JS/HTML strings | Legacy and Relay extractors (quotes, metadata, mutations), script/asset URL resolution, catalog layering, Relay `…Query` names, cache persistence + TTL, features from switches |
| Live (opt-in) | `twitty-x/tests/live.rs` `#[ignore]` | real x.com | Extractors over downloaded bundles, logged-out discovery crawl, real HomeTimeline call |
| Backend contract | `twitty-model/tests/contract.rs` (generic) | run against `MockBackend` and `XBackend`+wiremock | Both backends behave the same for paging, errors and actions |
| Persistence | `twitty/tests/persistence.rs` + `tests/golden/` | golden files | Every past `schema_version` loads and migrates. Saving then loading gives the same data. |
| App state | `twitty/src/state/**`, `account.rs` `#[cfg(test)]` | std, `MockBackend`, `MemoryStore` | `ColumnModel` paging/dedup/unseen/failures, `Interactions` toggles + rollback, `DeckFile`/`Settings`/credentials golden files + atomic save, `LoginForm` paste normalisation, keyring errors drop their payload, media grid geometry |
| UI wiring | `twitty/src/ui/**` `#[gpui_kit::test]` | `TestAppContext`, `MockBackend`, `Runtime::inline()` | Column loads on creation, list rows match the model, newer posts at top vs. scrolled, failure + retry, Home tab switch, hub rollback + `ActionFailed`, sign-in stores only accepted sessions and rebuilds columns, sign-out (keyring only), expired session opens the dialog |
| Live smoke | `twitty-x/tests/live.rs` `#[ignore]` | real X, `TWITTY_LIVE=1` | The current X still works: Viewer, Home, Search, TweetDetail translate with 0 `Unsupported`. Run by hand, never in CI. |

## Current coverage (M6)

233 tests (+3 opt-in live tests):
- 120 in `twitty-x`: 86 unit tests (session, errors, headers, cookies, rate limits, interpretation, catalog, discovery, anonymiser, wire, request mapping, text incl. a property test, users, media, cards, tweets, notifications), 16 transport and 13 backend mock-server tests, and 5 fixture translation tests.
- 23 in `twitty-model`: rich-text invariants (including a property test), stable `FeedSource` JSON, row keys, mock paging and notifications.
- 90 in `twitty`: state (paging, rate-limit/session hold-off, end-of-feed, settings, login form), credentials payload and stores, formatting, runtime, themes, icons, GPUI view tests (columns, deck scrolling, sign-in dialog, sign-in/out and the startup session check), and session discovery/rotation in `main.rs`. No test touches the real Credential Manager.

## Manual check

With a session (see "Signing in" in [x-api.md](x-api.md)), `cargo run -p twitty` shows real timelines; the log says `signed in as @handle`. Without one it shows the deck on demo data. New demo posts arrive every 20s and columns poll every 30s. After about a minute, a column scrolled away from the top shows the "N new posts" pill.

## Fixtures

- These live in `crates/twitty-x/tests/fixtures/<name>.json` and **must be anonymised** with `twitty-x-cli anonymise` (see [x-api.md](x-api.md#capturing-a-fixture)). Current set: `home`, `home--latest`, `usertweets-elonmusk`, `usertweets-nasa`, `search-quotes`, `search-videos`, `search-rust`, `tweetdetail`, `notifications`.
- Minimum cases, kept up to date:
  - plain post, reply, self-thread module, retweet, quote
  - quote of a deleted post, long `note_tweet`
  - 1/2/3/4 photos, video, GIF, poll, link card
  - tombstone, withheld, `TweetWithVisibilityResults`
  - pinned entry, promoted entry (dropped), top/bottom cursors, empty timeline
  - emoji/RTL/CJK text with entities.
- When X changes, **add** a new fixture (e.g. `retweet_2026-11.json`) and keep the old ones. Old shapes must keep translating if X still sends them anywhere.

## Snapshot policy

- Snapshots are of **model output**, never raw JSON. They're stable, readable and diffable.
- A snapshot diff on an existing fixture is a behaviour change. It must be intended and explained in the commit message.

## Test style

- Name tests `<unit>_<scenario>_<expected>`, e.g. `translate_retweet_unwraps_original_author`.
- Tests return `Result<(), Box<dyn Error>>` and use `?`. Asserts must carry useful messages.
- No network, no sleeping, and no wall clock in tests. Inject a `Clock` and use `tokio::time::pause` for backoff tests.
- Every bug found in production gets a test named after the symptom.

## Coverage

`cargo llvm-cov --workspace --html` (requires `cargo install cargo-llvm-cov`).
- Target **≥ 90%** line coverage for `twitty-x::translate`, `wire` and `transport`, and ≥ 80% across the workspace (excluding view rendering code).
- Coverage is a guide, not the goal. Behaviour tests come first.
