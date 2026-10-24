# Domain model (`twitty-model`)

## Why Twitty has its own model

X's GraphQL responses change often: fields move, `__typename`s appear, feature flags change the shape of a response. If those shapes spread through the app, every X change would break the UI, the caches and the saved settings.

So Twitty has an **anti-corruption layer**:

- `twitty-model` is the stable language the whole app speaks. It's designed around what Twitty **shows and does**, not around what X sends.
- `twitty-x::translate` is the only place that maps X's shapes onto it.
- When X changes, only `translate` (and its fixtures) changes. Model snapshots stay identical.

## Core types (planned)

| Type | Meaning |
|---|---|
| `PostId`, `UserId`, `ListId` | Opaque string newtypes. Never parsed outside the backend. |
| `PageToken` | Opaque paging cursor. Only the backend that produced it understands it. |
| `Profile` | User: handle, display name, avatar, banner, bio (`RichText`), badges, counts, relation to the viewer |
| `Post` | Author (`Arc<Profile>`), time, body (`RichText`), `Vec<Media>`, `Option<Embed>`, reply info, metrics, viewer state, canonical URL |
| `RichText` | `text: String` + `spans: Vec<Span>`. Spans are **byte ranges into `text`**, sorted and non-overlapping. |
| `SpanKind` | `Mention`, `Hashtag`, `Cashtag`, `Link { display, url }` |
| `Media` | `Image`, `Video`, `Gif`, each with an `ImageRef` that carries size hints (so the list can reserve height) |
| `Embed` | `Quote(PostOrGone)`, `LinkCard`, `Poll` |
| `FeedItem` | `Post`, `Repost { by, post }`, `Thread(Vec<Post>)`, `Notification`, `Unavailable`, `Unsupported { reason }` |
| `FeedPage` | `items` + `newer`/`older` page tokens |
| `FeedSource` | What a column shows: `Home`, `Notifications`, `Search`, `List`, `User`, `Bookmarks`, `Thread`. It's **persisted**. |
| `PostAction` | `Like`, `Unlike`, `Repost`, `Unrepost`, `Bookmark`, `Unbookmark`, … |
| `Draft` | Text, reply target, quote target, media attachments |
| `BackendError` | `Unauthorized`, `RateLimited { reset_at }`, `NotFound`, `Upstream`, `Network`, `Decode` |

The `Backend` trait (`viewer`, `feed`, `thread`, `profile`, `act`, `publish`) is the only way the app reaches a network.

## Translation rules (`twitty-x::translate`)

1. **Never panic and never fail a whole page.** An entry that can't be translated becomes `FeedItem::Unsupported { reason }`, plus a `tracing::warn!` that includes the entry ID and the missing path.
2. **Retweets are unwrapped**: the retweet becomes `FeedItem::Repost { by, post }`, where `post` is the original post.
3. **Long posts**: prefer `note_tweet` text and entities over the truncated `full_text`.
4. **Entities**: X gives offsets in **UTF-16 code units** (sometimes code points). These are converted to byte ranges in the decoded string. Out-of-range or overlapping entities are dropped with a warning, never clamped silently.
5. **Text clean-up**: decode HTML entities (`&amp;`, `&lt;`, …). Replace `t.co` links with their expanded URL. Strip the trailing media `t.co` link X appends to the text. Strip the leading `@mentions` in reply-chain text, because they show up as reply info instead.
6. **Media**: choose the highest-bitrate MP4 variant for video and keep the poster image. Read the aspect ratio from `original_info` / `aspect_ratio`.
7. **Unavailable posts** (`TweetTombstone`, `TweetUnavailable`, withheld, or a deleted quoted post) become `Unavailable { reason }` / `PostOrGone::Unavailable`.
8. **Times** become `DateTime<Utc>`, and counts are `u64` (a missing count is `0`, and the model has no way to say "unknown").
9. **Profiles are interned**: the same `UserId` within a page gets the same `Arc<Profile>`.

## Persistence and versioning

Saved files:
- `decks.json`: decks, columns, `FeedSource`, widths
- `settings.json`
- the optional post cache

Rules:
- Each file has a top-level `schema_version: u32`.
- Loading runs migrations `vN → vN+1` until it reaches the current version.
- Each version has a golden file in `crates/twitty/tests/golden/`, and loading it is a test.
- **Never** save X JSON or `PageToken`s that should outlive a session.
- Adding a field: `#[serde(default)]`, with no version bump. Renaming or removing a field or changing its meaning: bump the version and add a migration.
