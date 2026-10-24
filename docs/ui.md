# UI (`twitty` crate)

## Stack

- **GPUI** (the `gpui-pre =0.3.9` snapshot), used through **GPUI Kit** (`gpui-kit =0.8.0`), which re-exports gpui, `gpui-base` and `gpui-component`.
- Startup: `gpui_kit::application().run(|cx| { gpui_kit::init(cx); cx.set_http_client(..); gpui_kit::open_window(..) })`. `open_window` mounts a `Root`, which dialogs, sheets, notifications and popovers need.
- Component reference: `gpui-component-0.8.0/src/` in the cargo registry, and the gpui-component repo's `crates/story` gallery and `examples/`.

## GPUI patterns we follow

- **Views** are structs that implement `Render`. Child views are `Entity<T>` fields created with `cx.new(..)`. Small stateless pieces are `RenderOnce` components (e.g. `PostCell`, `MediaGrid`, `ActionBar`).
- **State updates**: change the fields, then call `cx.notify()`. Notify between views with `cx.emit` + `cx.subscribe` (EventEmitter), not callbacks that keep references.
- **Async work**: `cx.spawn(async move |this: WeakEntity<Self>, cx| { let r = runtime_handle.await; this.update(cx, |view, cx| { ...; cx.notify() }) })`. Store the returned `Task` in the view, so dropping the view cancels the work.
- **Lists**: each column owns a `ListState` (`gpui::list`), which measures rows of different heights and only renders what's visible. When items are added or removed, use `ListState::splice`. To keep the reading position when newer posts are inserted at the top, adjust the scroll offset by the new items' height. Use `ListState::reset` only on a full reload.
- **Images**: `img(url)` with a fixed size from `ImageRef.size` hints, so rows don't jump when images load. Show a placeholder until the image is ready.
- **Actions/keybindings**: declare them with `actions!(twitty, [...])` and bind them in `main.rs`, with key contexts per view (`Deck`, `Column`, `Composer`).

## X Pro look (spec)

Colours are from memory of x.com's palette: **check them against x.com's live CSS** before finalising the theme files.

```
┌──┬───────────────────┬───────────────────┬────────────
│𝕏 │ ⌂ Home   Following⚙│ 🔔 Notifications ⚙│ 🔍 query  ⚙   ← sticky column headers
│⌂ ├───────────────────┼───────────────────┼────────────
│🔍│ ◯ Name @h · 2m  ⋯ │                   │
│🔔│ body text …        │                   │
│☰ │ [ media grid ]    │                   │
│＋│ 💬 🔁 ♥ 📊 🔖 ↗   │                   │
│✎ │───────────────────│                   │
│◯ │                   │                   │
└──┴───────────────────┴───────────────────┴────────────
 rail   column (fixed width, horizontal scroll across columns)
```

| Element | Spec |
|---|---|
| Rail | ~68px wide, with icon buttons and tooltips. Top to bottom: logo, Home, Search, Notifications, Lists, Add column, Deck switcher. Bottom: Compose (accent), account avatar menu. |
| Deck | One horizontal scroll area. Columns are separated by 1px `border`. Columns can be reordered by dragging the header. |
| Scrollbars | Always visible (`Theme::set_scrollbar_mode(Always)` in `ui/theme.rs`). Each column has a vertical bar driven by its `ListState`. The deck has a horizontal bar in a 12px strip under the columns, driven by its `ScrollHandle`. |
| Column widths | Narrow 310px · Medium 350px (default) · Wide 400px |
| Column header | 53px tall: icon, title (bold), subtitle (gray, `@handle` / query), ⚙ popover (width, Top/Latest, media size, clear, remove) |
| Post cell | 12px padding · 40px round avatar · **name** · `@handle · 2m` gray · ⋯ menu · body 15px · media grid (1–4, 16px radius, 2px gap) · quote card (1px border, 16px radius, smaller avatar) · action bar (gray, active colours: like `pink`, repost `green`) |
| Sign-in | Demo banner with **Sign in** · rail account button (log-in icon in demo, account icon when signed in) · centred overlay dialog with how-to steps, masked `auth_token`/`ct0` inputs, Enter submits, "Checking…" while `viewer()` runs, error text from `LoginForm` · account panel: `@handle`, session source, **Sign out** for keyring sessions |
| Feedback | "↑ N new posts" pill when scrolled down · skeleton rows while loading · inline error with Retry (a full refresh after a session error, see `ColumnModel::retry_kind`) · footer row: spinner, Retry, "You're all caught up", or "Nothing to show here yet" for an empty feed · rate-limit countdown |
| Thread | Clicking a post opens a side `sheet` with the conversation |
| Composer | `dialog`, with a character counter, reply and quote preview, and image attachments (later) |
| Video/GIF | Poster image with a ▶ badge and duration. Click opens it externally (GPUI has no video player). |

### Themes

These are gpui-kit theme JSON files in `crates/twitty/themes/`, following gpui-component's `.theme-schema.json`.

| Token | Lights out | Dim | Light |
|---|---|---|---|
| background | `#000000` | `#15202B` | `#FFFFFF` |
| border | `#2F3336` | `#38444D` | `#EFF3F4` |
| foreground | `#E7E9EA` | `#F7F9F9` | `#0F1419` |
| muted foreground | `#71767B` | `#8B98A5` | `#536471` |
| accent / primary | `#1D9BF0` | `#1D9BF0` | `#1D9BF0` |

Views read colours from `cx.theme()` only. Never hard-code hex in a view.

Font: Segoe UI / system UI. Chirp, X's own font, isn't bundled.

## Polling

Each visible column polls for `Newer` posts. The base interval is 60s for Home and 30s for Notifications. It grows when the operation's rate-limit budget is low, and stops for hidden or minimised windows.

## Keybindings (planned)

| Key | Action |
|---|---|
| `j` / `k` | Next / previous post in the focused column |
| `h` / `l` (or ←/→) | Previous / next column |
| `Enter` | Open thread |
| `r` | Reply · `t` repost · `f` like · `b` bookmark |
| `n` | New post |
| `Esc` | Close sheet / dialog |
| `.` | Load new posts / scroll to top |
