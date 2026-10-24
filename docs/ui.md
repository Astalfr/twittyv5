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
- **Actions/keybindings**: declared in `ui/keys.rs` with `actions!(twitty, [...])` and bound by `keys::bind` (called from `main.rs` and keyboard tests). Deck shortcuts live in the `Deck` key context on the rail/columns/thread container; dialogs are rendered outside it (with an `Overlay` context that only binds Esc), so typing in a text field never triggers a shortcut. When no dialog is open, `TwittyApp::render` gives focus back to the deck.

## X Pro look (spec)

Colours are from memory of x.com's palette: **check them against x.com's live CSS** before finalising the theme files.

```
┌──┬───────────────────┬───────────────────┬────────────
│🐦│ ⌂ Home   Following⚙│ 🔔 Notifications ⚙│ 🔍 query  ⚙   ← sticky column headers
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
| Rail | ~68px wide, with icon buttons and tooltips. Top to bottom: Twitty logo (`ui::assets::LOGO`), **+ Add column**. Bottom: account button, settings, Compose (accent). |
| Deck bar | 36px strip above the columns: one tab per deck (active one underlined, with × when more than one deck exists) and **+** for a new empty deck ("Deck N"). Switching rebuilds the columns of that deck; the hub (likes etc.) is kept. |
| Add column | Overlay dialog: a button per kind (Home, Notifications, Search, User, List, Post, Bookmarks). Kinds without input are added at once; others take a query, `@handle`/profile link, list link/id or post link/id (parsed in `state::add_column`). Handles are resolved with `Backend::profile` ("Looking up…"), unknown ones show an error. |
| Column tabs | Under the header, from `state::sources::tabs`: Home For you/Following · Notifications All/Verified/Mentions · Search Top/Latest/Media · User Posts/Replies/Media/Likes. A tab switches the column's source and is saved in the deck. |
| Deck | One horizontal scroll area. Columns are separated by 1px `border`. Columns can be reordered by dragging the header. |
| Scrollbars | Always visible (`Theme::set_scrollbar_mode(Always)` in `ui/theme.rs`). Each column has a vertical bar driven by its `ListState`. The deck has a horizontal bar in a 12px strip under the columns, driven by its `ScrollHandle`. |
| Column widths | Narrow 310px · Medium 350px (default) · Wide 400px |
| Column header | 53px tall: icon, title (bold), subtitle (gray, `@handle` / query), ⚙ popover (width, Top/Latest, media size, clear, remove) |
| Post cell | 12px padding · 40px round avatar · **name** · `@handle · 2m` gray · ⋯ menu · body 15px · media grid (1–4, 16px radius, 2px gap) · quote card (1px border, 16px radius, smaller avatar) · action bar (gray, active colours: like `pink`, repost `green`) |
| Sign-in | Demo banner with **Sign in** · rail account button (log-in icon in demo, account icon when signed in) · centred overlay dialog with how-to steps, masked `auth_token`/`ct0` inputs, Enter submits, "Checking…" while `viewer()` runs, error text from `LoginForm` · account panel: `@handle`, session source, **Sign out** for keyring sessions |
| Feedback | "↑ N new posts" pill when scrolled down · skeleton rows while loading · inline error with Retry (a full refresh after a session error, see `ColumnModel::retry_kind`) · footer row: spinner, Retry, "You're all caught up", or "Nothing to show here yet" for an empty feed · a rate-limited load shows "rate limited until <time>" and the column waits for that time |
| Image viewer | Clicking a media tile opens it full-window with `n / count · @handle`, ‹ › (and ←/→) for multi-image posts (wrapping), alt text, and **Play on x.com** for videos and GIFs. Esc, × or a click on the backdrop closes it (buttons and the image stop their clicks); the backdrop is the shared overlay backdrop (see Backdrop). The image box is computed from the window size (`media_layout::viewer_box`, with fixed-height header and footer and the alt text clamped to two lines) so nothing is pushed off screen; the `img` has no explicit size, so GPUI uses the decoded bitmap's size capped by that box: images shrink to fit but are never scaled up past their own resolution. |
| Settings | Rail ⚙ opens a page: theme (Lights out / Dim / Light), "Check for new posts automatically", the keyboard shortcuts, and where data is saved. Saved to `settings.json` right away. |
| Active column | The column keyboard shortcuts act on has a tinted header; its selected row has the `list.active.background` tint (the primary blue at ~12–19% opacity, per theme) and a 3px `list.active.border` accent bar on the left, which stays visible while the row's hover background covers the tint. |
| Backdrop | Every overlay (image viewer, dialogs, settings, account panel) and the thread panel sits on the theme's `overlay` colour at 70% opacity (`BACKDROP_ALPHA` in `ui/app.rs`). |
| Thread | Clicking a post (or a quoted post) opens `ThreadPanel`, as a 600px sheet over the right side of the deck, on the backdrop: ancestors and the focused post as one connected thread, then replies (`state::thread::rows`). Clicking a post inside it switches the panel; × or a click on the backdrop closes it. The backdrop is occluding and takes the whole click, so it doesn't also open the post, image or link under it. While an overlay (image viewer, compose) is open over the panel, the panel's backdrop isn't drawn, so the overlay's darkens everything once. A reply published from compose reloads the panel showing its parent. |
| Links | Mentions, hashtags, cashtags and links use the theme's `link` colour; the one under the mouse is underlined (`post::span_at` maps the hovered byte to a span; the hover is kept in `window.use_keyed_state` and cleared when the mouse leaves the text). A click opens it on x.com / in the browser. |
| Action bar | Reply (opens compose) · Repost · Like · Views (no action) · Bookmark · Share (opens the post on x.com). Toggles are optimistic and roll back on error. Clicks on links, media, cards and actions stop propagation so they don't also open the thread. |
| Compose | Overlay dialog from the rail's pen button or Reply: auto-growing textarea, "Replying to @x" with the parent's text, `remaining / 280` counter using X's weighted length (`state::compose`: links 23, CJK/emoji 2; warning under 20, danger below 0), Post disabled when empty or too long, X's error shown on failure with the text kept. |
| Profile hover card | Hovering an author's avatar shows a card (avatar, name, @handle, bio, following/followers) with **Open column**, which adds a User column. |
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

Each column polls for `Newer` posts at an interval from `state::poll`:
- polling may use a third of a feed's budget (`Backend::rate_budget`, e.g. X's 15-minute limit for the operation), shared by all columns of the same feed type (`poll::bucket`);
- never more often than every 30s, never less often than every 30 min; conversations aren't polled;
- `TwittyApp::update_polling` recomputes intervals when the deck changes and every minute (X reports its real limits with the first responses; before that `XBackend` assumes a limit of 50);
- the settings page can turn polling off; a rate-limited or unauthorised column also holds off (`ColumnModel::begin`).

## Keyboard shortcuts

| Key | Action |
|---|---|
| `j` / `k` (or ↓/↑) | Next / previous post in the active column |
| ← / → | Previous / next column (in the image viewer: previous / next image) |
| `Enter` | Open the conversation |
| `l` · `t` · `b` | Like · repost · bookmark |
| `r` | Reply |
| `n` | New post |
| `Esc` | Close the image viewer, a dialog, or the thread panel |
