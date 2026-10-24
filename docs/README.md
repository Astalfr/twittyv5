# Twitty documentation

These docs explain **how Twitty works**. They change with the code: any change to architecture, data flow, protocol handling, persisted formats or testing updates the matching page in the same commit.

> Project status: **M0–M10 done** (all planned milestones). Hardening: per-operation health counters in Settings → Diagnostics, a rotating log file, logged panics. Like, retweet, bookmark, reply and tweet write to Twitter (tested against a mock server; not yet exercised on a real account); clicking a tweet opens its conversation in a side panel. Sign in by pasting two cookies (kept in Windows Credential Manager); every column type loads from Twitter (Home, Notifications, Search, User posts/replies/media/likes, Lists, Bookmarks, a tweet's conversation); columns are added from a dialog and arranged in several decks. Without a session it runs on demo data. Deferred by decision: Twitter's real transaction-id algorithm and TweetDeck deck sync (see decisions.md).

| Page | What it covers |
|---|---|
| [architecture.md](architecture.md) | Crates, layers, dependency rules, async/runtime model, data flow from click to pixels |
| [domain-model.md](domain-model.md) | `twitty-model` types, the `Backend` trait, translation rules, persistence and versioning |
| [x-api.md](x-api.md) | How `twitty-x` talks to Twitter: auth, headers, query IDs, features, transaction ID, rate limits, parsing, **"Twitter broke something" runbook** |
| [ui.md](ui.md) | GPUI / GPUI Kit patterns, the TweetDeck look, layout spec, theming, list rendering, keybindings |
| [testing.md](testing.md) | Test layers, fixtures, snapshots, property tests, UI tests, live tests, definition of done |
| [development.md](development.md) | Setup, commands, toolchain, dependency pinning, upgrading GPUI Kit, debugging tips |
| [decisions.md](decisions.md) | Log of architecture decisions (lightweight ADRs) |

## Writing docs

- Explain **why** and **how parts fit together**. Leave out line-by-line code descriptions; the code and `///` docs cover those.
- Prefer short sections, tables and small diagrams.
- Link to code with relative paths (`../crates/twitty-x/src/translate/mod.rs`).
- If a doc and the code disagree, the code is right. Fix the doc.
