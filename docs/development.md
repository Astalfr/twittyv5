# Development

## Requirements

- Windows 11 (main platform). macOS and Linux should work through GPUI but aren't a goal.
- Rust stable ≥ 1.98, pinned in `rust-toolchain.toml`, edition 2024.
- MSVC build tools (Visual Studio Build Tools, "Desktop development with C++") and the Windows SDK.
- Optional tools: `cargo install cargo-insta cargo-llvm-cov cargo-deny cargo-nextest`.

## Commands

| Task | Command |
|---|---|
| Fast check | `cargo check --workspace --all-targets` |
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Test | `cargo test --workspace` (or `cargo nextest run --workspace`) |
| Review snapshots | `cargo insta review` |
| Run the app | `cargo run -p twitty` (`RUST_LOG=twitty=debug,twitty_x=debug` for logs) |
| Dev CLI (from M3) | `cargo run -p twitty-x --bin twitty-x-cli -- <feed\|capture\|anonymise\|refresh-ids> …` |
| Live smoke tests (from M3) | `$env:TWITTY_LIVE=1; cargo test -p twitty-x --test live -- --ignored` |
| Coverage | `cargo llvm-cov --workspace --html` |
| Dependency audit | `cargo deny check` |

## Workspace conventions

- Shared versions and lints live in the root `Cargo.toml` (`[workspace.dependencies]`, `[workspace.lints]`). Crates use `dep.workspace = true` and `lints.workspace = true`.
- Workspace lints:
  - `rust`: `unsafe_code = "forbid"` (relaxed only in `twitty` if ever needed)
  - `clippy`: `unwrap_used`, `expect_used`, `dbg_macro`, `todo`, `unimplemented` = deny; `print_stdout`/`print_stderr` = warn
- `[profile.dev.package]` builds `gpui-pre*` with `opt-level = 3`, so debug builds stay smooth, as gpui-component does.
- `clippy.toml` allows `unwrap`/`expect`/`print` in tests only.
- `rustfmt.toml`: `max_width = 100`, `use_small_heuristics = "Max"`.

## Icons

Icons come from GPUI Kit's Lucide catalog. Only the ones listed in `crates/twitty/src/ui/assets.rs` (`icon_assets!`) are embedded. A new icon must be added there, or it renders blank. The `every_used_icon_resolves` test checks a sample.

## Pinning and upgrading GPUI Kit

- `gpui-kit = "=0.8.0"` pins `gpui-pre =0.3.9`. `gpui-pre-reqwest-client` must use the **same** snapshot version.
- `Cargo.lock` is committed. Never run `cargo update` across the board. Update specific crates with `cargo update -p <crate>`.
- To upgrade:
  1. Read gpui-component's changelog and release notes.
  2. Bump `gpui-kit` and every `gpui-pre-*` pin together.
  3. Fix compile errors.
  4. Run the full test suite and click through every column type by hand.
  5. Record the upgrade in [decisions.md](decisions.md).
  6. Do the whole upgrade in **its own commit**.

## Local data

| Data | Location |
|---|---|
| Credentials (`auth_token`, `ct0`) | Windows Credential Manager, service `twitty` |
| Decks, settings | `%APPDATA%\Twitty\config\` (`decks.json`) |
| Query-ID cache, image cache, logs | `%LOCALAPPDATA%\Twitty\` |

## Debugging tips

- `TWITTY_DUMP_DIR=<dir>` makes `twitty-x` write every raw response it receives (with secrets redacted). This is useful for capturing fixtures of new shapes.
- In debug builds, the debug panel shows per-operation rate-limit budgets and counts of `Unsupported` items and decode warnings.
- gpui-component's `inspector` feature (`gpui-kit/inspector`) gives a GPUI element inspector, similar to browser DevTools.
