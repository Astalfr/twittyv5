# Development

## Requirements

- Windows 11 (main platform). macOS and Linux should work through GPUI but aren't a goal.
- Rust stable ≥ 1.98, pinned in `rust-toolchain.toml`, edition 2024.
- The WebView2 runtime, for the sign-in window (built into Windows 11; without it the dialog falls back to pasting cookies).
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
| Dev CLI | `cargo run -p twitty-x --bin twitty-x-cli -- <ops\|refresh-ids\|capture home\|capture op\|anonymise> …` (see [x-api.md](x-api.md#dev-cli)) |
| Live smoke tests (from M3) | `$env:TWITTY_LIVE=1; cargo test -p twitty-x --test live -- --ignored` |
| Coverage | `cargo llvm-cov --workspace --html` |
| Dependency audit | `cargo deny check` |

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main`/`master`, on `v*` tags, on pull requests and on demand (`workflow_dispatch`). `check` and `build` run on `windows-latest`; `release` runs on tags only:

| Job | Steps |
|---|---|
| `check` | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` |
| `build` | `cargo build --release --locked -p twitty`, uploads `target/release/twitty.exe` as the artifact `twitty-windows-x64-<sha>`, then builds the installer and uploads it as `twitty-setup-windows-x64-<sha>` (both kept 30 days) |
| `release` | Tags `v*` only, after `check` and `build` pass (`ubuntu-latest`, `contents: write`): downloads both artifacts, fails unless the installer is named after the tag's version, then `gh release create <tag>` with the installer and `twitty.exe` and generated notes |

- The toolchain comes from `rust-toolchain.toml`. `Swatinem/rust-cache` caches debug and release builds separately. Only `main` saves caches, and it saves them even when a step fails or times out (`cache-on-failure`). The compile steps have their own `timeout-minutes`, below the job's, so the save step still runs. Pull requests restore `main`'s cache.
- CI builds without debug info (`CARGO_PROFILE_DEV_DEBUG=0`) and turns off Defender real-time scanning first. Runs on `main` are never cancelled by a newer push (that would discard their cache); pull request runs are.
- The first run after changing a `CARGO_*` variable in the workflow, `Cargo.lock` or the toolchain starts cold (new cache key).
- `--locked` fails the build if `Cargo.lock` is out of date, so commit the lockfile with every dependency change.
- GPUI tests use GPUI's test platform, so CI needs no real display.
- Download the `.exe` from the run's **Artifacts** section on GitHub.
- **Installer**: `installer/twitty.iss` is an [Inno Setup](https://jrsoftware.org/isinfo.php) script. CI installs Inno Setup with Chocolatey when the runner image lacks it, reads the version from `cargo metadata` and runs `ISCC.exe /DAppVersion=<version> installer\twitty.iss`, which writes `target/installer/twitty-setup-<version>-x64.exe`. To build it locally: `winget install JRSoftware.InnoSetup`, build the release exe, then run the same command (the script refuses to compile without `AppVersion`). It installs per user under `%LOCALAPPDATA%\Programs\Twitty` without admin rights (the wizard offers an all-users install), adds a Start menu shortcut and an optional desktop one, and closes a running Twitty before upgrading. Uninstalling leaves settings, decks, logs and the saved session in place. `AppId` identifies the app for upgrades and uninstall: never change it. The installer isn't signed, so SmartScreen warns on first run. Run silently with `/relaunch=1` (what the updater does), it starts Twitty when it's done.

### Releases and self-update

To publish a version:

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`), run `cargo check` so `Cargo.lock` follows (CI builds with `--locked`), and commit both.
2. Tag the commit `v<version>` (e.g. `git tag v0.2.0`) and push the tag (`git push origin v0.2.0`). A tag that doesn't match the version fails the `release` job.

Installed copies then offer it on their next check:

- `main.rs` enables updates only when the exe was installed by the installer (`unins000.exe` next to it); `cargo run` and copied exes never check.
- The app calls `GET https://api.github.com/repos/Astalfr/twittyv5/releases/latest` (unauthenticated: 60 requests an hour per IP) 10 s after startup, then every 6 hours, unless "Look for new versions on GitHub" is off. A 404 (no release yet), a draft or a pre-release offers nothing; other failures are logged. A check or download fails after 30 s without data (`update::STALL_TIMEOUT`, connect and read timeouts, so a slow download that keeps going isn't cut).
- The release must carry `twitty-setup-<version>-x64.exe` with GitHub's `digest` (`sha256:…`, set on every uploaded asset). **Install and restart** downloads it to `%LOCALAPPDATA%\Twitty\cache\updates\`, checks the SHA-256, runs it with `/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /relaunch=1`, waits for pending settings and deck writes and quits. The installer keeps the previous folder and install mode, and starts the new version.
- The digest only catches a corrupt download: whoever can publish a release can ship code. Releases aren't signed ([decisions.md](decisions.md)).
- Release builds use the Windows GUI subsystem (no console window). Debug builds keep the console for logs.
- **App icon**: `crates/twitty/build.rs` compiles `resources/twitty.rc` with `embed-resource` (the Windows SDK's `rc.exe`, present on `windows-latest`), embedding `resources/twitty.ico` as icon resource 1. Explorer uses it for the exe, and GPUI loads resource 1 as the window and taskbar icon. The build fails if the resource can't be compiled. `resources/twitty-logo.png` (256px, the `.ico`'s 256px frame) is the title bar logo, embedded through `ui::assets` as `brand/twitty.png` (`LOGO_PNG`), and the icon of Windows notifications (saved to the cache folder and registered as the app id's `IconUri`). GPUI samples images bilinearly with no mipmaps, so a PNG much bigger than its drawn size (`ui::assets::LOGO_SIZE`, 18px) aliases: the title bar draws it through `ui::remote_image::asset`, which resamples it on the CPU to the device size for the window's scale factor (a test checks it at 100–300%). Both were generated from the 512px source PNG with high-quality resampling. The `.ico` holds PNG frames from 16 to 256px.

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

## Emoji font

`crates/twitty/resources/fonts/Twemoji.Mozilla.ttf` is Mozilla's COLR build of Twemoji, patched by `fill_base_glyphs.py` in the same folder (the README there has the source, checksums and licence). To update it, download the new release, run the script on it, and check that `ui::emoji`'s tests pass: `emoji_glyphs_have_outlines` fails on an unpatched font.

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

The app's files live in three folders (`twitty::dirs::AppDirs`):

| Folder | Release build | Debug build (`cargo run`, tests) |
|---|---|---|
| config | `%APPDATA%\Twitty\config\` | `dev-data\config\` at the repo root |
| cache | `%LOCALAPPDATA%\Twitty\cache\` | `dev-data\cache\` |
| data | `%LOCALAPPDATA%\Twitty\data\` | `dev-data\data\` |

Development never touches the installed app's decks, settings, caches or logs. `dev-data/` is gitignored and safe to delete. The path is fixed at compile time, so a debug exe copied elsewhere still writes into its checkout. The session is kept apart too: debug builds use their own Credential Manager entry (`Twitty` / `x-session-dev`), so signing in or out with `cargo run` leaves the installed app's session alone. `twitty-x-cli` keeps using the per-user cache.

| Data | Location |
|---|---|
| Credentials (`auth_token`, `ct0`) | Windows Credential Manager, entry `Twitty` / `x-session` (`x-session-dev` in debug builds; versioned JSON) |
| Sign-in window's WebView2 profile | `cache\webview2` (InPrivate: no cookies are kept; safe to delete) |
| Decks, settings | `config\` (`decks.json`, `settings.json`) |
| Query-ID cache, client ids | `cache\` (`query-ids.json`, `client-ids.json`) |
| Update downloads | `cache\updates\` (installed copies only) |
| Log | `data\logs\twitty.log` (+ `twitty.log.1`), rotated at startup past 5 MB |
| Dev session (optional) | `session.json` at the repo root or in the config folder (gitignored) |

## Debugging tips

- Logs go to the console in debug builds and always to the log file (release builds have no console). `RUST_LOG` overrides the default `twitty=info,twitty_x=info`. Panics are logged before the default handler runs.
- Settings → **Diagnostics** shows, per Twitter operation since startup: responses, rows, rows Twitty couldn't read (`Unsupported`), errors, and the last warning/error. "Unreadable" rows are the signal that Twitter changed its JSON.
- To capture a raw response for a new shape, use `twitty-x-cli capture` (see [x-api.md](x-api.md#dev-cli)).
- gpui-component's `inspector` feature (`gpui-kit/inspector`) gives a GPUI element inspector, similar to browser DevTools.
