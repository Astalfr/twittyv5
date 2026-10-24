# Twitty

A native X (Twitter) client in Rust + GPUI (via GPUI Kit). It uses X's internal web GraphQL API and has an X Pro / TweetDeck-style multi-column UI.

- Roadmap and milestones: [PLAN.md](PLAN.md)
- How the project works: [docs/](docs/README.md). Read the relevant doc before changing a subsystem.

## Commands

```sh
cargo check --workspace --all-targets                 # fast feedback loop
cargo fmt --all                                       # before finishing any change
cargo clippy --workspace --all-targets -- -D warnings # must be clean
cargo test --workspace                                # must pass
cargo insta review                                    # review changed snapshots (never blindly accept)
cargo run -p twitty                                   # run the app
# From M3 on (not implemented yet):
# cargo run -p twitty-x --bin twitty-x-cli -- feed home   # dump a real feed as model JSON
# cargo test -p twitty-x --test live -- --ignored         # live smoke tests (TWITTY_LIVE=1)
```

Windows 11 + PowerShell is the main dev environment.

## Architecture rules (do not break)

- **Crates**:
  - `twitty-model` holds the stable domain types and the `Backend` trait. It does no I/O and has no GPUI.
  - `twitty-x` is the X adapter that implements `Backend`.
  - `twitty` is the GPUI app.
  - Details are in [docs/architecture.md](docs/architecture.md).
- **IMPORTANT: X's JSON shapes never leave `twitty-x`.**
  - `twitty_x::wire` is `pub(crate)`, and only `twitty_x::translate` converts wire → model.
  - The UI, the caches and persisted files use `twitty-model` types only.
  - Never add X field names (`legacy`, `rest_id`, `__typename`, …) to `twitty-model`.
  - See [docs/domain-model.md](docs/domain-model.md).
- **Translation fails soft.** An entry that can't be parsed becomes `FeedItem::Unsupported` plus a `tracing::warn!`. It must never fail the whole page or panic.
- **Opaque IDs**: X cursors, query IDs and IDs stay wrapped (`PageToken`, `PostId`, `UserId`). The UI never parses them.
- **Versioned persistence**: every on-disk format has a `schema_version` and a migration path. Changing a persisted type means adding a migration and a golden-file test.
- **The UI depends on `dyn Backend`**, never on `XBackend` directly. UI work and UI tests use `MockBackend` (`twitty-model` feature `mock`).
- **Logic lives in `twitty::state`** (plain Rust, unit tested); `twitty::ui` views only render state and forward input.

## Rust practices

- Edition 2024, stable toolchain (pinned in `rust-toolchain.toml`). No nightly features.
- **Errors**:
  - Libraries (`twitty-model`, `twitty-x`) use `thiserror` enums.
  - The binary uses `anyhow` with `.context("…")`.
  - Propagate with `?`.
- **No `unwrap()`/`expect()` in non-test code.** The only exception is `main()` setup, with a message that explains the invariant. This is enforced by `clippy::unwrap_used` / `expect_used` in workspace lints.
- No `panic!`, `todo!` or `unimplemented!` on code paths that run.
- **Unsafe code**: `#![forbid(unsafe_code)]` in `twitty-model` and `twitty-x`. Anywhere else, `unsafe` needs a `// SAFETY:` comment and must be discussed first.
- **Lint suppressions**: every `#[allow(...)]` needs a comment explaining why. Prefer `#[expect(...)]`.
- **Types**: use newtypes and enums instead of `String`/`bool` flags. Make invalid states unrepresentable. Derive `Debug, Clone, PartialEq` on model types, and `Serialize, Deserialize` where they're persisted.
- **Ownership**: borrow (`&str`, `&[T]`) in arguments, and own in structs. Share large immutable data with `Arc<T>` (e.g. `Arc<Profile>`). No gratuitous `.clone()` to get past the borrow checker; restructure instead.
- **Visibility**: `pub(crate)` by default. `pub` only for items that are deliberately part of the API, with `///` docs.
- **Imports**: no wildcard imports, except `gpui_kit::*` / prelude imports in UI modules and `use super::*` in tests.
- **Dependencies**: add them to `[workspace.dependencies]` and justify each one in the commit message. Don't add a crate for something std does in a few lines.
- **Logging**: `tracing` only, no `println!` (except in the CLI binary).
- **IMPORTANT: never log, print, snapshot or commit `auth_token`, `ct0` or cookies.** Secret types implement a redacting `Debug`.

## GPUI / GPUI Kit specifics

- **Use `gpui-kit = "=0.8.0"`**, which pins the `gpui-pre =0.3.9` snapshot. Never depend on the `gpui` crate from crates.io. Keep `Cargo.lock` committed. Upgrading is a separate, deliberate change ([docs/development.md](docs/development.md)).
- **The GPUI API changes between snapshots, so don't guess signatures.** Check the source in `~/.cargo/registry/src/*/gpui-pre-0.3.9` / `gpui-component-0.8.0`, or the gpui-component `examples/` and `crates/story`.
- **GPUI's executor is not tokio.**
  - Network calls run on the tokio runtime in `twitty::runtime` and are awaited from `cx.spawn`.
  - Never block the UI thread or call `block_on` inside GPUI.
- **Spawned tasks** hold `WeakEntity`, never `Entity`. Don't keep entity borrows across `.await`. Call `cx.notify()` after mutating view state.
- **Timelines** use `list()` + `ListState` (variable height). Don't use `uniform_list` / `v_virtual_list` for posts.
- **`ListState::set_scroll_handler` runs while the `ListState` is mutably borrowed.** Reading the list there panics, so `cx.defer` the reaction (see `ui/column.rs`).
- **`overflow_hidden` clips to a rectangle, not to rounded corners.** Images inside rounded boxes must round their own outer corners (`rounded_img` + `media_layout::Corners` in `ui/post.rs`).
- **Horizontal-only scroll containers** need `.restrict_scroll_to_axis()`, or GPUI turns vertical wheel input into horizontal scrolling (see the deck in `ui/app.rs`).
- **Colours** come from the theme (`cx.theme()` tokens), never hard-coded hex in views. UI conventions: [docs/ui.md](docs/ui.md).

## Testing: a feature isn't done until it's tested

Full strategy: [docs/testing.md](docs/testing.md).

- **Every change ships with tests, in the same change.** Bug fixes start with a failing test that reproduces the bug.
- **`translate` changes**: add or update an anonymised fixture in `crates/twitty-x/tests/fixtures/` plus an `insta` snapshot of the *model* output. The model snapshot of an existing fixture must not change unless the change is intended and explained.
- **Text/entity code** (UTF-16 offsets, spans, URL expansion) gets `proptest` property tests (spans in bounds, on char boundaries, not overlapping).
- **Transport behaviour** (headers, cookie rotation, 429 backoff, stale queryId refresh) is tested with `wiremock`. Tests never hit the real network, except the `#[ignore]` live suite.
- **UI state logic** goes in `twitty::state` with plain unit tests. View wiring is tested with `#[gpui_kit::test]` + `MockBackend` + `Runtime::inline()` (see the tests in `ui/column.rs`).
- **Persisted formats** have golden files for every past `schema_version`, and those files must keep loading.

**Before saying a task is done**, run all of the following and show the output:
1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`

Don't weaken, `#[ignore]` or delete a test to make it pass. Fix the root cause.

## Docs

- **Keep `docs/` accurate.** Any change to architecture, data flow, the X protocol handling, persisted formats or test strategy updates the matching doc in the same change.
- **Non-obvious decisions** are recorded in [docs/decisions.md](docs/decisions.md).
- **When X breaks** (stale query IDs, new fields, error 353, …), follow the runbook in [docs/x-api.md](docs/x-api.md) and record what changed.

## Git

- CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests with `--locked`, and builds the release `.exe`. Keep it green.

- Small, focused commits in imperative mood ("Add search column"). Run fmt, clippy and tests before every commit.
- Never commit secrets, real session files, or un-anonymised fixtures (they contain real users' data).
