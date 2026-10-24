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
- **Remote images in fixed boxes** (media tiles, quote and link-card images, profile gallery) use `ui::remote_image::source(url, w, h)`, never `img(url)`. GPUI draws bitmaps with no mipmaps, so a 1200 px photo shrunk into a 400 px tile looks jagged. `remote_image` fetches and decodes the image itself and shrinks it on the CPU (Catmull-Rom) to just cover the box in device pixels, rounded up to a bucket (`state::image_scale`, steps of 1.25×) so resizing a column doesn't reload every image; while a new size loads, the previous copy stays on screen. Images are never enlarged. The image viewer still uses `img(url)` because it sizes from the bitmap's own size. Embedded images drawn much smaller than their bitmap (the title bar logo) use `remote_image::asset(path, w, h)`, which does the same from the app's asset source.
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
| Title bar | No system title bar (`TitleBar::window_options()` in `main.rs`). `ui::app::render_title_bar` draws GPUI Kit's `TitleBar` (34px) in the theme's background, with no border of its own: the logo (`ui::assets::LOGO`, 18px, resampled to the device size by `remote_image::asset`) and "Twitty" (small text; the foreground colour in dark themes, muted in light ones) centred on the window (a layer without handlers, so drags reach the bar), and on the right the minimise / maximise / close buttons, which Windows treats as real caption buttons (Snap layouts, drag to move, double-click to maximise). Dialogs, the image viewer and the thread panel sit below it, so it stays usable while they're open. |
| Chrome border | The title bar and the rail have no borders; the deck area (demo banner, deck bar, columns) has a top and left border with an 8px rounded top-left corner (`CHROME_RADIUS`), so one line runs under the title bar and down the rail. |
| Rail | ~68px wide, with icon buttons and tooltips, no border. Top to bottom: one button per column (its source icon, title as tooltip, the active column selected; this list scrolls when there are many columns). Clicking one makes that column active and scrolls the deck just far enough to show the whole column. Then **+ Add column**. Bottom: account button, settings, Compose (accent). |
| Deck bar | 36px strip above the columns: one tab per deck (active one underlined, with × when more than one deck exists) and **+** for a new empty deck ("Deck N"). Switching rebuilds the columns of that deck; the hub (likes etc.) is kept. |
| Add column | Overlay dialog: a button per kind (Home, Notifications, Search, User, List, Post, Bookmarks). Kinds without input are added at once; others take a query, `@handle`/profile link, list link/id or post link/id (parsed in `state::add_column`). Handles are resolved with `Backend::profile` ("Looking up…"), unknown ones show an error. |
| Column tabs | Under the header, from `state::sources::tabs`: Home For you/Following · Notifications All/Verified/Mentions · Search Top/Latest/Media · User Posts/Replies/Media/Likes. A tab switches the column's source and is saved in the deck. |
| Deck | One horizontal scroll area. Columns are separated by 1px `border`. Columns can be reordered by dragging the header. |
| Scrollbars | Always visible (`Theme::set_scrollbar_mode(Always)` in `ui/theme.rs`). Each column has a vertical bar driven by its `ListState`. The deck has a horizontal bar in a 12px strip under the columns, driven by its `ScrollHandle`. |
| Column widths | Narrow 310px · Medium 350px (default) · Wide 400px |
| Column header | 53px tall: icon, title (bold), subtitle (gray, `@handle` / query), ⚙ popover (width, Top/Latest, media size, clear, remove) |
| Post cell | 12px padding · 40px round avatar · **name** · `@handle` gray, relative time (`2m`) at the far right of the header (a long name or handle truncates, the time never does; hovering the time underlines it and shows the exact local date and time in a tooltip (`format::exact_time`, 24-hour `2026-10-24 15:45`), clicking it opens the post's vxtwitter link, `state::links::vx_link`) · right-click anywhere on the row: context menu with **Copy tweet link** (copies the vxtwitter link) · ⋯ menu · body 15px · media grid (1–4, 16px radius, 2px gap) · quote card (1px border, 16px radius, smaller avatar; clicking its image opens the image viewer, not the quoted post) · action bar (gray, active colours: like `pink`, repost `green`) |
| Sign-in | Demo banner with **Sign in** · rail account button (log-in icon in demo; when signed in, the viewer's 30px round avatar, resampled to its drawn size with `remote_image::source`, or an account icon if they have none) · centred overlay dialog with how-to steps, masked `auth_token`/`ct0` inputs, Enter submits, "Checking…" while `viewer()` runs, error text from `LoginForm` · account panel: `@handle`, session source, **Sign out** for keyring sessions |
| Feedback | "↑ N new posts" pill when scrolled down · skeleton rows while loading · inline error with Retry (a full refresh after a session error, see `ColumnModel::retry_kind`) · footer row: spinner, Retry, "You're all caught up", or "Nothing to show here yet" for an empty feed · a rate-limited load shows "rate limited until <time>" and the column waits for that time |
| Image viewer | Clicking a media tile opens it full-window with `n / count · @handle`, ‹ › (and ←/→) for multi-image posts (wrapping), alt text, and **Play on x.com** for videos and GIFs. Esc, × or a click on the backdrop closes it (buttons and the image stop their clicks); the backdrop is the shared overlay backdrop (see Backdrop). The image box is computed from the window size (`media_layout::viewer_box`, with fixed-height header and footer and the alt text clamped to two lines) so nothing is pushed off screen; the `img` is given the decoded bitmap's size (read from GPUI's image cache) shrunk to fit that box with its aspect ratio kept (`media_layout::viewer_image_size`): images shrink to fit but are never scaled up past their own resolution. The size must be exact: with only `max_w`/`max_h`, GPUI clamps each side on its own and letterboxes the picture inside a larger element, whose bars then swallow clicks meant for the backdrop. |
| Settings | Rail ⚙ opens a page: theme (Lights out / Dim / Light), "Check for new posts automatically", "Animations", the keyboard shortcuts, and where data is saved. Saved to `settings.json` right away. |
| Active column | The column keyboard shortcuts act on has a tinted header; its selected row has the `list.active.background` tint (the primary blue at ~12–19% opacity, per theme), with no accent bar. |
| Backdrop | Every overlay (image viewer, dialogs, settings, account panel) and the side sheet (thread or profile) sits on the theme's `overlay` colour at 70% opacity (`BACKDROP_ALPHA` in `ui/app.rs`). It fades in and out with them (see Animations); over the thread panel it doesn't, so the deck never flashes lighter or darker. |
| Thread | Clicking a post (or a quoted post) opens `ThreadPanel`, as a 600px sheet over the right side of the deck, on the backdrop: ancestors and the focused post as one connected thread, then replies (`state::thread::rows`). Clicking a post inside it switches the panel; × or a click on the backdrop closes it. The backdrop is occluding and takes the whole click, so it doesn't also open the post, image or link under it. While an overlay (image viewer, compose) is open over the panel, the panel's backdrop isn't drawn, so the overlay's darkens everything once. A reply published from compose reloads the panel showing its parent. |
| Links | Mentions, hashtags, cashtags and links use the theme's `link` colour; the one under the mouse is underlined (`post::span_at` maps the hovered byte to a span; the hover is kept in `window.use_keyed_state` and cleared when the mouse leaves the text). A click on a mention opens the account's profile sheet (`Navigate::Mention`: the app looks the handle up with `Backend::profile` first, and falls back to x.com if the lookup fails); hashtags, cashtags and links open on x.com / in the browser. |
| Action bar | Reply (opens compose) · Repost · Like · Views (no action) · Bookmark, spread across the row with at least 8px between them. Counts are compact (`1.2K`); hovering one shows the full value in a tooltip (`1,234 Likes`, `format::exact_count`). Toggles are optimistic and roll back on error. Like and repost turn their active colour (red, green) on hover; the other actions turn blue. Clicks on links, media, cards and actions stop propagation so they don't also open the thread. |
| Compose | Overlay dialog from the rail's pen button or Reply: auto-growing textarea, "Replying to @x" with the parent's text, `remaining / 280` counter using X's weighted length (`state::compose`: links 23, CJK/emoji 2; warning under 20, danger below 0), Post disabled when empty or too long, X's error shown on failure with the text kept. |
| Profile hover card | Hovering an author's avatar shows a card (avatar, name, @handle, bio, following/followers) with **Open column**, which adds a User column. |
| Open a profile | Clicking an author's avatar, name or @handle opens the profile sheet instead of the post's thread. |
| Profile sheet | `ui::profile::ProfilePanel`, in the same side-sheet slot as the thread panel (one replaces the other; opening the account already shown does nothing). Header: 72px avatar, name + badge, @handle, bio, Tweets / Following / Followers counts. It shows the post's copy of the profile at once, then refreshes it with `Backend::profile`. Below, tabs **Tweets** and **Media**, each half the sheet's width: **Tweets** (the user's posts as normal rows), **Media** (a 3-wide gallery of square tiles: pictures, GIFs and videos with their badge; a click opens the image viewer). Each tab is a `ColumnModel` of `FeedSource::User { tab: Posts / Media }`, loaded the first time it's shown and paged as it scrolls; header, tabs, rows and footer are all rows of one `list()` (mapping in `state::profile`). The **+** button adds the account as a User column. |
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

## Animations

Durations and easing come from the theme's `MotionTokens` (`theme.motion`: enter = `duration_normal` + `easing_enter`, exit = `duration_fast` + `easing_exit`); row and pop timings and curves are in `state::motion` (unit and property tested).

| What | Motion |
|---|---|
| Rows arriving | A fresh page (first load, refresh, tab switch) cascades in: each of the first rows fades and rises 8px, 35ms after the one above. Newer posts at the top drop in from above with a primary tint that fades over 1.4s; older rows at the bottom fade in. Only paint moves (`opacity`, relative `top`), never the measured height, so `ListState` and scroll anchoring are unaffected. |
| "New posts" pill | Drops in from the column's top edge. |
| Like / repost / bookmark | Turning one on makes its icon "pop" (grow ~35% and settle) inside a fixed 17px slot. |
| Dialogs, settings, account | Backdrop fades in; the card fades and rises 8px. On close it fades and sinks while the backdrop fades out. |
| Image viewer | Fades in and out; stepping to another image fades it in. It doesn't move, so its buttons are where they end up from the first frame. |
| Thread panel / profile sheet | Fades in while sliding 48px from the right edge (short, so its hitbox is nearly in place at once); the backdrop fades in only when the panel opens, not when it switches posts or an overlay over it closes. Closing reverses it. |

Rules:
- **Entering** uses `with_animation` on the element as it mounts. GPUI animations run on the wall clock, so in tests they sit at their first frame: only fades and short offsets for anything tests click on.
- **Leaving**: GPUI has no unmount animation. Closing clears the state at once (`thread_panel()`, `compose()`, `media_viewer()` … are `None` immediately) and moves what was shown into a `leaving` slot on `TwittyApp`, drawn only to animate out and cleared by a timer task. The leaving layer is inert: no key context, no handlers, no occluding backdrop, and a transparent occluding cover over its card / panel (the image viewer is re-rendered without handlers), so clicks beside it reach the deck but nothing inside it reacts. Opening the same kind of thing again drops the leaving copy.
- **Rows**: a virtualised `list()` drops element state when a row scrolls out, so a plain `with_animation` would replay every time. `ColumnView` remembers arrivals in a `motion::Recent` keyed by item key with an expiry; a row animates only while its entry is live.
- **Off switch**: Settings → "Animations" sets GPUI's `reduce_motion` (GPUI never sets it from Windows' own setting). `with_animation` then renders end states, and the app skips its leaving layers, arrivals and pops. It is app-wide, so gpui-component's spinners and skeleton shimmer stand still too.
- A leaving copy is wrapped in a different animation id than the live view, so state keyed by element id inside it starts over. Anything visible that depends on such state (a scroll offset) lives in a handle instead (`settings_scroll`; the thread panel's `ScrollHandle`), and inner enter animations are skipped in the copy (the viewer's image fade).

## Polling

Each column polls for `Newer` posts at an interval from `state::poll`:
- polling may use a third of a feed's budget (`Backend::rate_budget`, e.g. X's 15-minute limit for the operation), shared by all columns of the same feed type (`poll::bucket`);
- never more often than every 30s, never less often than every 30 min; conversations and Home "For you" aren't polled ("For you" is re-ranked on every request, so it only refreshes when asked);
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
| `Esc` | Close the image viewer, a dialog, or the side sheet (thread or profile) |
