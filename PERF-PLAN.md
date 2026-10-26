# Build speed, CI time, binary size and RAM plan

Goals: faster local compiles, shorter CI (≈6.5 min wall time today), a smaller
`twitty.exe` (42 MB release), lower RAM use at runtime.

Each change below ships as its own commit with tests and the matching doc
update (`docs/development.md`, `docs/decisions.md`, `docs/testing.md`), as
CLAUDE.md requires.

## Starting point

CI timings of the last two `main` runs (warm caches):

| Job | Wall time | Main costs |
|---|---|---|
| `check` | 3.7–4.1 min | cache restore 60 s, clippy 57–66 s, tests 70–79 s |
| `build` | **6.5 min** | **release build 311–324 s**, cache restore 35 s |

- The release build is the critical path. Speeding up `check` doesn't change wall time until it is fixed.
- `twitty-x`'s `reqwest 0.13` `rustls` feature pulls in **aws-lc-rs / aws-lc-sys** (a slow C build). GPUI's reqwest fork already uses **ring**, so two crypto providers are compiled and shipped.
- Release exe 42 MB, debug exe 73 MB. The emoji font is 1.7 MB of it. Most duplicate deps come from GPUI and can't be fixed here (windows 0.58/0.62, resvg/usvg 0.45/0.46, png, zune-jpeg, gif, exr); `sha2 0.10` is ours.
- `clap` and `tracing-subscriber` are normal deps of `twitty-x` because of `twitty-x-cli`, so every `twitty` build compiles them.
- No doctests exist, yet `cargo test` runs rustdoc on each lib. `twitty-x` (4) and `twitty-media` (2) have one test binary per file, each linked separately.
- RAM grows without bound:
  - every `remote_image` load goes through `window.use_asset::<Scaled>`; GPUI's `loading_assets` never evicts, so each image is kept forever, once per size bucket;
  - `Latest` (`ui/remote_image.rs`) is never pruned, and atlas textures are never dropped;
  - `ColumnModel.items` (`state/column.rs`) only grows (polling + "load older").

## Phase 0: measure (no code changes)

1. Split the release build time: touch `crates/twitty`, `cargo build --release -p twitty --timings`. Deps recompiling vs `twitty` codegen vs LTO + link.
2. Size breakdown: `cargo bloat --release --crates -p twitty`.
3. RAM baseline: Working Set and Private Bytes after ~10 min of real use with ~4 columns, sampled every 30 s by a small PowerShell sampler (manual use, no automation).
4. Feature ping-pong: does `cargo build -p twitty` recompile `gpui-pre` after `cargo test --workspace`?

Results go in the "Phase 0 results" section at the end of this file.

## Phase 1: low-risk wins

| # | Change | Helps | Risk / check |
|---|---|---|---|
| 1 | `twitty-x` reqwest: aws-lc-rs → ring (`rustls-no-provider` + install ring's provider) | cold builds, cache misses, size | Live suite covers TLS. decisions.md entry. |
| 2 | `rust-lld` linker (`.cargo/config.toml`, MSVC target) | linking locally and in CI | Icon + manifest from `embed-resource` must still embed. |
| 3 | `[profile.dev.package."*"] debug = false`, workspace crates keep `limited` | local link time, PDB size | none |
| 4 | `doctest = false` on the four lib crates | test step | none (no doctests) |
| 5 | One integration-test binary per crate (`tests/it/main.rs`); `live` may stay separate | fewer links | update testing.md commands |
| 6 | `twitty-x-cli` deps behind a `cli` feature + `required-features` | `twitty` builds skip clap | update CLI commands in CLAUDE.md / x-api.md |
| 7 | `sha2` 0.11 if it removes the duplicate digest stack | a bit of everything | check with `cargo tree -d` |

### Phase 1 outcome (2026-10-26)

| # | Result |
|---|---|
| CI | **Done.** `build` is skipped on pull requests (Phase 2's PR decision, done early). |
| 1 | **Not possible.** `gpui-pre-http-client-tls` depends on rustls' default features (aws-lc) and installs aws-lc as the process default itself, so aws-lc is built and shipped whatever reqwest uses. Reverted. |
| 2 | **No gain, dropped.** rust-lld: `build -p twitty` after touching `twitty` 7.7 s either way; `test --workspace --no-run` 14.7–15.5 s vs 11.3–12.0 s with link.exe. |
| 3 | **No gain, dropped.** The PDB went from ~180 MB to 83 MB, but builds were no faster (8.3–9.6 s, 12–17.6 s). Linking isn't the bottleneck. |
| 4 | **Done.** Saves ~2 s per `cargo test` locally, more on CI runners. |
| 5 | **Skipped.** Each integration-test binary runs in under 0.4 s and links without GPUI; merging them would rename every insta snapshot for a gain of a second or two. |
| 6 | **Skipped.** clap only compiles on a cold build; CI restores it from cache. Not worth the extra `--features cli` on every CLI command. |
| 7 | **Done.** sha2 0.11 is the one rust-embed already builds, so the 0.10 stack (sha2, digest, block-buffer, crypto-common) is no longer built on Windows. No code change. |

What's left in a warm incremental dev build: `twitty` lib 1.7 s (frontend 1.2 s, codegen 0.5 s), `twitty` bin 6 s. The bin step is `main.rs` codegen plus the link. Running the tests takes 15 s locally, 7.7 s of it the `twitty` lib's 557 tests.

## Phase 2: CI restructure

Decided: PRs skip the release build, and tag builds must not get slower.

- **PRs:** the `build` job runs only on pushes to `main`, on `v*` tags and on `workflow_dispatch` (`if: github.event_name != 'pull_request'`). PR wall time becomes the `check` job alone (~4 min today).
- **`main` and tags keep the same `release` profile (thin LTO).**
  - Tags restore `main`'s release cache, so a different profile on tags would start them cold and make them slower.
  - `main`'s build keeps that cache warm. Nobody waits on it.
- **No `dist` profile, no fat LTO and no `codegen-units = 1`.** Each would slow tag builds.
- **The `check` job is now the PR critical path.**
  - Phase 1 (rust-lld, merged test binaries, no doctests, ring) targets it directly.
  - Then re-measure the 60 s cache restore.
  - Optional: clippy and test as parallel jobs (≈1 min).
- **Docs:** update the CI section of development.md, and record "PRs don't build the exe" in decisions.md.

## Phase 3: binary size (only settings that don't slow the build, measured with `cargo bloat`)

- `opt-level = "s"` per package for crates off the render path (wry, tao, reqwest/rustls, regex, clap, tracing-subscriber, twitty-x). gpui, image, twitty-media stay at 3. Check scrolling smoothness by hand.
- `panic = "abort"`: optional, decisions.md entry. A panic in a tokio task would kill the app instead of becoming a `JoinError`.
- Not changeable: the font (compressing it costs RAM), GPUI's image codecs. The installer is already LZMA-compressed.

### Phase 2/3 outcome (2026-10-26)

- **CI:** `check` is split into parallel `lint` (fmt + clippy, cache `clippy`) and `test` (cache `test`) jobs; `release` needs all three jobs. The first `main` runs under the new cache keys start cold.
- **Size:** `opt-level = "s"` for ~30 crates off the hot path took the release exe from 42,591,744 to 39,979,008 bytes (−6.1 %). Incremental release build 57 s (was 59 s). `twitty-x` at `"s"` saved another 0.2 MB but stays at 3 (it translates every page).

## Phase 4: RAM (logic in `twitty::state`, unit tested)

Priority lowered: the 10-minute baseline is flat (see results).

- First, sample a long session (2–4 h with polling) to see whether the unbounded caches actually grow.
- Then do item 1 or item 2 only if that sample shows growth.

1. Bounded image cache: `state::image_cache`, an LRU with a byte budget (e.g. 256 MB of decoded BGRA), touched when drawn. Eviction calls `cx.remove_asset::<Scaled>`, drops the atlas texture with `drop_image` (as `ui/player.rs` does) and prunes `Latest`.
2. Column trimming: cap items per column (e.g. ~500), only when scrolled near the top. Drop whole pages and keep the bottom page's cursor (`PageToken` is opaque). Property tests: no gap and no duplicate after "load older".
3. Video: frame queue depth in `video.rs` / `ui/player.rs`, and release decoder + buffers when a player leaves the screen.
4. Optional: mimalloc, only if the baseline shows a gain.

## Later, depending on Phase 0

- `twitty::state` as its own crate so state tests skip GPUI (only `deck.rs` and `sources.rs` mention gpui). Needs an architecture.md update.
- Local environment: separate rust-analyzer target dir (`rust-analyzer.cargo.targetDir: true`), Dev Drive or Defender exclusion for `target/` and `~/.cargo`.

## Decisions

1. PRs skip the release build.
2. Tag builds must not get slower for a smaller exe, so there is no fat LTO and no `codegen-units = 1`.

## Phase 0 results

Measured 2026-10-26 on the dev machine (24 threads). CI runners have 4 vCPUs, so the CPU-bound times scale up there.

### Release build time

| Build | Time | Notes |
|---|---|---|
| Release, deps warm, all 4 workspace crates rebuilt | 73 s | |
| Release after touching `crates/twitty` | **59 s** | `twitty` lib 9.7 s (frontend 4.4 s, codegen 5.3 s); **`twitty` bin 47.4 s** |
| Full release rebuild (profile change) | ~290 s | |

- **About 80 % of an incremental release build is the final `twitty` bin step.** With `lto = "thin"` that step runs cross-crate thin LTO over the whole dependency graph, then links. It is the cost CI pays on every push, even with a perfect cache, and on 4 vCPUs it is much slower. The compiled `twitty` code itself is only ~10 s.
- **Conclusion:** turning LTO off for non-tag builds (Phase 2) is the top lever for CI time. Phase 1 items speed up cold builds and the check job, but not this tail.

### Feature ping-pong

- None. After `build -p twitty` → `test --workspace --no-run`, a second round of each rebuilt 0 crates (1 s each).
- Dropped from the plan.

### Binary size (42.6 MB)

PE sections:

| Section | Size |
|---|---|
| `.text` | 27.8 MiB |
| `.rdata` | 11.1 MiB |
| `.pdata` | 1.2 MiB |
| `.data` + `.reloc` + `.rsrc` | 0.5 MiB |

Top crates in `.text` (`cargo bloat --crates`):

| Crate | Size |
|---|---|
| std | 3.3 MiB |
| gpui | 3.2 MiB |
| gpui_base | 3.0 MiB |
| gpui_component | 2.3 MiB |
| **twitty** | 1.6 MiB |
| image | 1.1 MiB |
| usvg | 0.96 MiB |
| reqwest | 0.71 MiB |
| gpui_windows | 0.65 MiB |
| rustls | 0.63 MiB |
| image_webp | 0.61 MiB |
| regex_automata | 0.50 MiB |
| twitty_x | 0.46 MiB |
| gpui_pre_reqwest (second HTTP stack) | 0.38 MiB |
| exr (unused codec, enabled by GPUI) | 0.34 MiB |
| aws_lc_sys | 0.19 MiB |
| ring | 0.12 MiB |

- **Our own code is small.** `twitty` + `twitty_x` + `twitty_model` add up to ~2.2 MiB; the rest is GPUI and its dependencies. Size gains must come from profile settings, not from code changes. With fat LTO and `codegen-units = 1` ruled out, that leaves per-package `opt-level = "s"`, so expect modest gains.
- **The ring switch saves little size.** aws-lc's code is only ~0.2 MiB, so that item is mainly about compile time.
- **Lucide icons are not an issue (dropped).** 172 SVGs are embedded, not gpui-kit-assets' full catalog of 1831, so `AllAssets` is not linked.
- **The only embedded font is Twemoji (1.6 MiB).** The other ~9.5 MiB of `.rdata` is constant data and tables spread over the dependencies. `cargo bloat` can't attribute it.
- **Upstream weight we can't remove:** `markdown` + `html5ever` (~0.34 MiB, gpui-component), `exr` + `tiff` (~0.46 MiB, GPUI's image features), and two HTTP stacks.

### RAM baseline

10.5 min of manual use, sampled every 30 s:

| | Start | After 30 s | 1–10.5 min |
|---|---|---|---|
| Private bytes | 178 MB | 216 MB | flat, 216–218 MB |
| Working set | 109 MB | 158 MB | 152–158 MB |
| Handles | 1363 | ~1850 | ~1725 |
| Threads | 37 | 44 | 37–39 |

- **No growth over 10 minutes.** The app loads its first pages and images, then stays flat at ~216 MB private.
- **What this sample can't show:**
  - GPU atlas textures live in video memory, which these counters don't include;
  - the unbounded caches (images, `Latest`, column items) would grow over hours of polling and scrolling, not minutes.
- **Next check:** a long session (2–4 h) before any Phase 4 code.
- **Optional, in the same session:** compare the "Dedicated/Shared GPU memory" columns in Task Manager's Details tab.
