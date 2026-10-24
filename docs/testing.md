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
| Translation snapshots | `twitty-x/tests/translate.rs` + `tests/fixtures/*.json` | `insta` (text summaries) | 13 anonymised live responses translate with **zero** `Unsupported` rows and zero warnings · compact per-row summaries are snapshotted · fixtures together must cover reposts, threads, notifications, quotes, link cards, video, GIF, multi-image, replies, long posts and mention/hashtag/link spans · promoted posts skipped · authors shared per page · media grid pages (incl. `TimelineAddToModule`) carry posts with media |
| Wire robustness | `twitty-x/tests/wire_robustness.rs` | `proptest`, mutated fixtures | Missing or renamed fields and unknown typenames never panic, and become `Unsupported` |
| Entity offsets | `twitty-x/src/translate/text.rs` tests | `proptest` | Code-point → byte conversion with emoji and CJK, display ranges, media-link removal, HTML decoding. A property test feeds random entities and ranges: never panics, spans always valid. |
| Backend | `twitty-x/tests/backend.rs` | `wiremock`, committed fixtures as bodies | Each `FeedSource` calls its operation and translates with 0 `Unsupported` · older/newer cursors sent · search over POST with `product` · 401 → `Unauthorized` · a media column pages through the grid · each action posts its mutation with the right id field · "already done" (139/327) is success · other X errors are reported · publish sends the draft (reply id) and returns the created post, duplicates are errors · rate budget uses X's reported limit · health counts rows, unreadable rows (with the reason), errors, and responses without a timeline · thread split around the focal post, missing focal → `NotFound` · `Viewer`/`UserByScreenName` → `Profile` · writes refused without a request |
| Transport | `twitty-x/tests/transport.rs` | `wiremock`, `tokio::test` | Required headers present · `x-csrf-token == ct0` · fresh txid per request · cookie rotation used next + hook · 429 reset + local gate per operation · 5xx retried, 401 not · 404 → discovery → retry once with discovered features · override skips discovery · POST body for mutations · cursor forwarded · secrets never in `Debug`/errors |
| Discovery + catalog | `twitty-x/src/{discovery,catalog}.rs` unit tests | tiny synthetic JS/HTML strings | Legacy and Relay extractors (quotes, metadata, mutations), script/asset URL resolution, catalog layering, Relay `…Query` names, cache persistence + TTL, features from switches |
| Live (opt-in) | `twitty-x/tests/live.rs` `#[ignore]` | real x.com | Extractors over downloaded bundles, logged-out discovery crawl, real HomeTimeline call |
| Backend contract | `twitty-model/tests/contract.rs` (generic) | run against `MockBackend` and `XBackend`+wiremock | Both backends behave the same for paging, errors and actions |
| Video decoding | `twitty-media/tests/{decode,audio}.rs` + `tests/fixtures/{bars,tone}.mp4`, `src/lib.rs` | Media Foundation, std | `tone.mp4` (1 s, AAC stereo 44.1 kHz, a 440 Hz tone on the left, silence on the right): float stereo at the file rate · video and sound in time order, about a second of sound · the tone stays left · a seek starts both tracks at the target (frames on screen then, sound within a millisecond) with the channels in order · a video-only decoder skips the sound · a silent file has no sound track · exact-seek helpers: frames before the target skipped, sound trimmed in whole frames · `bars.mp4` (8 frames, 128 × 90 coded as 96 rows, a different colour on top of each; both files written by `cargo run -p twitty-media --example make_fixture`) decodes in order at the right times, cropped to 90 rows, upright, BGRA with opaque alpha · rewind starts over · duration · missing files and garbage are `Open` errors · failed downloads (a local 404 server) leave later playbacks working · decoders run on other threads · without Media Foundation (GitHub's Server runners) each test prints that it skipped · the row copier flips bottom-up rows, crops, and refuses short buffers instead of reading past them |
| Persistence | `twitty/tests/persistence.rs` + `tests/golden/` | golden files | Every past `schema_version` loads and migrates. Saving then loading gives the same data. |
| App state | `twitty/src/state/**`, `account.rs` `#[cfg(test)]` | std, `MockBackend`, `MemoryStore` | `ColumnModel` paging/dedup/unseen/failures, `Interactions` toggles + rollback, `DeckFile` (decks add/switch/remove, v1 → v2 width migration, clamped widths/volumes, blank names)/`Settings`/credentials golden files + atomic save, `LoginForm` paste normalisation, column tabs, "Add column" input parsing (handles, list/post links), weighted post length (incl. a property test), thread rows, polling intervals (stay within a third of the budget, sharing per feed type), keyboard selection (clamping, follows prepended rows), animation timing (`state::motion`: arrivals expire, cascade stagger, frames in range incl. a property test), diagnostics verdict, log file rotation, keyring errors drop their payload, media grid geometry, video frame timing (`state::playback`: first frame at once, late ticks skip, the clock waits for a slow decoder, pause/resume, bounded queue, inline GIF play rules: setting until the viewer chooses, the button does the opposite, quality labels by height or bitrate), settings golden v1 has GIF autoplay off the sound clock (`audio`: chunks play in order and set the clock, a dry queue plays silence and stops it, pausing doesn't consume, volume is a gain, a seek drops old sound mid-chunk, a device that stops pulling stops being the clock, stop ends the source), picture following the sound and the wall clock carrying on when it stops, seeks (preview while paused, continue while playing, to the very end, a preview arriving with the end) and the end, and the decode loop (`video`: loops keep the clock growing, sound queued beside the frames, hanging up stops the decoder, a seek reaches a decoder blocked on a full channel, a seek after the end plays again, dropping the control silences the sound at once) |
| UI wiring | `twitty/src/ui/**` `#[gpui_kit::test]` | `TestAppContext`, `MockBackend`, `Runtime::inline()` | Column loads on creation, list rows match the model, newer posts at top vs. scrolled, failure + retry, Home tab switch, hub rollback + `ActionFailed`, sign-in stores only accepted sessions and rebuilds columns, sign-out (keyring only), expired session opens the dialog, add-column dialog (immediate kinds, search, handle lookup, unknown user, bad links), deck switching, thread panel (load, retry), compose (post, reply, too long/empty, failure keeps text), navigation from rows (thread, reply, profile column; avatar/name clicks open the profile, not the thread), profile sheet (opens instead of a column, same account keeps it, Media tab gallery opens the viewer, a post replaces it with its thread), polling spread and switched off, real keystrokes (j/k/←/→, l, Enter, Esc, n; typing in compose isn't a shortcut), image viewer stepping and closing, settings saved (theme, polling, animations), leaving copies of closed overlays and the thread panel are inert and cleared after their exit (`advance_clock`), rows arriving and toggle pops (and none with reduced motion), column settings dialog (live apply, saved on release and on close, volume slider only with the sound, the Test button sends a notification), new-post alerts (sound via `RecordingSound`, system notifications via GPUI's test platform; not for the first page or a refresh), header buttons shown only while hovered, video player (`FakeVideo`: the due frame is drawn and frames off screen leave the sprite atlas, release frees the last one, click pauses, no source fails soft) a GIF playing in the viewer until it steps away or closes (a click pauses and resumes, not the backdrop), a GIF that fails to play bringing back "Play on x.com", inline GIFs in a row (off by default: nothing downloads until the button; the button plays, pauses and resumes without opening the viewer; autoplay plays on its own and a pause beats it; the rest of the tile opens the viewer; a tile no longer drawn hangs up its playback, `FakeVideo::listening`), the GIF autoplay setting applies live and is saved, the video player (`FakeVideo`/`FakeControl`: GIFs ask for no sound and draw no bar; videos ask for sound and their bar plays and pauses the sound too; a seek shows its target and ignores older frames; at the end play starts over; mute and volume set the gain and carry to the next video), the bar staying up while either slider is held (released over nowhere), the volume slider keeping off the right edge, switching quality (reopens on that file, seeks to where it was, keeps the pause, drops the old playback; the same quality doesn't reopen), no quality menu with one file, the bar's buttons hover with a clear light surface, a quoted video or GIF showing its badge, a video in the viewer (sound, the model's duration, the bar, no x.com button, the bar's button toggles once and keeps the viewer, closing stops the playback; a seek drag released over the picture or the backdrop only seeks, once) |
| Live smoke | `twitty-x/tests/live.rs` `#[ignore]` | real X, `TWITTY_LIVE=1` | The current X still works: Viewer, Home, Search, TweetDetail translate with 0 `Unsupported`. Run by hand, never in CI. |

## Current coverage (M10)

462 tests (+3 opt-in live tests):
- 133 in `twitty-x`: 91 unit tests (incl. health counters), 16 transport and 20 backend mock-server tests, and 6 fixture translation tests over 13 fixtures.
- 24 in `twitty-media`: the row copier (crop, bottom-up rows, short buffers), exact-seek helpers, COM start-up, and 14 decoder tests over the synthetic `bars.mp4` and `tone.mp4` (skipped where Media Foundation is missing, as on CI).
- 25 in `twitty-model`: rich-text invariants (including a property test), stable `FeedSource` JSON, row keys, mock paging and notifications.
- 280 in `twitty`: state (paging, hold-off, end-of-feed, selection, polling, animation timing, diagnostics, settings, login form, decks, tabs, add-column parsing, compose length, thread rows), credentials, logging, formatting, runtime, themes, icons, GPUI view tests, and session discovery/rotation in `main.rs`. No test touches the real Credential Manager or writes to X.

## Manual check

With a session (see "Signing in" in [x-api.md](x-api.md)), `cargo run -p twitty` shows real timelines; the log says `signed in as @handle`. Without one it shows the deck on demo data. New demo posts arrive every 20s and columns poll every 30s. After about a minute, a column scrolled away from the top shows the "N new posts" pill.

## Fixtures

- These live in `crates/twitty-x/tests/fixtures/<name>.json` and **must be anonymised** with `twitty-x-cli anonymise` (see [x-api.md](x-api.md#capturing-a-fixture)). Current set: `home`, `home--latest`, `usertweets-elonmusk`, `usertweets-nasa`, `search-quotes`, `search-videos`, `search-rust`, `tweetdetail`, `notifications`, `userreplies-nasa`, `usermedia-nasa`, `usermedia-nasa-2`, `list-space`.
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
