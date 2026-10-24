# Decision log

Lightweight ADRs: newest first. Each entry: date, decision, why, consequences. Add one whenever a choice isn't obvious from the code, or when X forces a change.

---

### 2026-10-24: Own domain model with an anti-corruption layer
- **Decision**: All X JSON is parsed in a private `twitty_x::wire` module and converted into `twitty-model` types by `twitty_x::translate`. Nothing outside `twitty-x` sees X's structures.
- **Why**: X's internal schema changes often. Keeping those changes in one module protects the UI, persisted data and tests from breaking.
- **Consequences**: Translation code has to be written. Snapshot tests are on model output, so X drift appears as a `translate` diff rather than app-wide breakage.

### 2026-10-24: GPUI through GPUI Kit, pinned exactly
- **Decision**: Depend on `gpui-kit = "=0.8.0"` (`gpui-pre =0.3.9`) and commit `Cargo.lock`.
- **Why**: GPUI snapshots break the API. gpui-kit provides the 70+ components we need (avatar, popover, sheet, dialog, tabs, …) and a theme system.
- **Consequences**: Upgrades are deliberate, separate changes ([development.md](development.md)).

### 2026-10-24: Separate tokio runtime for networking
- **Decision**: `twitty-x` uses reqwest on a tokio runtime that runs on a background thread. The UI awaits its `JoinHandle`s from GPUI tasks.
- **Why**: GPUI's executor isn't tokio, and keeping `twitty-x` independent of GPUI makes it testable with plain `#[tokio::test]`.

### 2026-10-24: `list()`/`ListState` for timelines
- **Decision**: Use GPUI's `list` element (measured, variable height) for columns.
- **Why**: Post heights vary. `uniform_list` and `v_virtual_list` need heights up front.

### 2026-10-24: Login by pasting cookies (v1)
- **Decision**: The user pastes `auth_token` + `ct0`. They're stored in Windows Credential Manager.
- **Why**: `gpui-webview` doesn't support Windows, and building the X login flow ourselves is fragile and risky.

### 2026-10-24: Random `x-client-transaction-id` for now
- **Decision**: Send a random 32-hex value per request, as `bun_x` does.
- **Why**: X currently accepts it. The real algorithm is a known follow-up, triggered by error 353 ([x-api.md](x-api.md)).

### 2026-10-24: `bun_x` as reference, not as a dependency
- **Decision**: Write our own small `twitty-x` and borrow ideas and patterns from `bun_x` (Apache-2.0, with credit).
- **Why**: `bun_x` is ~14k lines and does far more than we need. We want to control the wire→model boundary.
