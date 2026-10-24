# Twitty documentation

These docs explain **how Twitty works**. They change with the code: any change to architecture, data flow, protocol handling, persisted formats or testing updates the matching page in the same commit.

> Project status: **planning**. Pages describe the intended design from [PLAN.md](../PLAN.md). Mark sections `(planned)` until they're implemented, and remove the marker when the code exists.

| Page | What it covers |
|---|---|
| [architecture.md](architecture.md) | Crates, layers, dependency rules, async/runtime model, data flow from click to pixels |
| [domain-model.md](domain-model.md) | `twitty-model` types, the `Backend` trait, translation rules, persistence and versioning |
| [x-api.md](x-api.md) | How `twitty-x` talks to X: auth, headers, query IDs, features, transaction ID, rate limits, parsing, **"X broke something" runbook** |
| [ui.md](ui.md) | GPUI / GPUI Kit patterns, the X Pro look, layout spec, theming, list rendering, keybindings |
| [testing.md](testing.md) | Test layers, fixtures, snapshots, property tests, UI tests, live tests, definition of done |
| [development.md](development.md) | Setup, commands, toolchain, dependency pinning, upgrading GPUI Kit, debugging tips |
| [decisions.md](decisions.md) | Log of architecture decisions (lightweight ADRs) |

## Writing docs

- Explain **why** and **how parts fit together**. Leave out line-by-line code descriptions; the code and `///` docs cover those.
- Prefer short sections, tables and small diagrams.
- Link to code with relative paths (`../crates/twitty-x/src/translate/mod.rs`).
- If a doc and the code disagree, the code is right. Fix the doc.
