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
| Translation snapshots | `twitty-x/tests/translate.rs` + `tests/fixtures/**` | `insta` (JSON/RON) | X JSON → `FeedPage` is exactly what we expect |
| Wire robustness | `twitty-x/tests/wire_robustness.rs` | `proptest`, mutated fixtures | Missing or renamed fields and unknown typenames never panic, and become `Unsupported` |
| Entity offsets | `twitty-x/src/translate/text.rs` tests | `proptest` | UTF-16 → byte conversion for emoji, ZWJ sequences, CJK, combining marks. Spans are in bounds, on char boundaries, sorted and non-overlapping. |
| Transport | `twitty-x/tests/transport.rs` | `wiremock`, `tokio::test` | Required headers present · `x-csrf-token == ct0` · cookie rotation saved · 429 backoff · rate-limit gate · 404 → queryId refresh → retry once · secrets never in logs |
| Discovery | `twitty-x/tests/discovery.rs` | captured HTML/JS samples | Bundle URL and `queryId`/`operationName`/features regexes · cache TTL · fallback order |
| Backend contract | `twitty-model/tests/contract.rs` (generic) | run against `MockBackend` and `XBackend`+wiremock | Both backends behave the same for paging, errors and actions |
| Persistence | `twitty/tests/persistence.rs` + `tests/golden/` | golden files | Every past `schema_version` loads and migrates. Saving then loading gives the same data. |
| UI state | `twitty/src/**` `#[gpui::test]` | `TestAppContext`, `MockBackend` | Column paging, new-posts pill count, optimistic like + rollback on error, deck add/remove/reorder, login flow states, rate-limit UI |
| Live smoke | `twitty-x/tests/live.rs` `#[ignore]` | real X, `TWITTY_LIVE=1` | The current X still works: Viewer, Home, Search, TweetDetail translate with 0 `Unsupported`. Run by hand, never in CI. |

## Fixtures

- These live in `crates/twitty-x/tests/fixtures/<operation>/<case>.json` and **must be anonymised** with `twitty-x-cli anonymise` (see [x-api.md](x-api.md#capturing-a-fixture)).
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
