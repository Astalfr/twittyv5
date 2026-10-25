# Domain model (`twitty-model`)

## Why Twitty has its own model

Twitter's GraphQL responses change often: fields move, `__typename`s appear, feature flags change the shape of a response. If those shapes spread through the app, every Twitter change would break the UI, the caches and the saved settings.

So Twitty has an **anti-corruption layer**:

- `twitty-model` is the stable language the whole app speaks. It's designed around what Twitty **shows and does**, not around what Twitter sends.
- `twitty-x::translate` is the only place that maps Twitter's shapes onto it.
- When Twitter changes, only `translate` (and its fixtures) changes. Model snapshots stay identical.

## Core types (planned)

| Type | Meaning |
|---|---|
| `PostId`, `UserId`, `ListId` | Opaque string newtypes. Never parsed outside the backend. |
| `PageToken` | Opaque paging cursor. Only the backend that produced it understands it. |
| `Profile` | User: handle, display name, avatar, banner, bio (`RichText`), badges, counts, relation to the viewer |
| `Post` | Author (`Arc<Profile>`), time, body (`RichText`), `Vec<Media>`, `Option<Embed>`, reply info, metrics, viewer state, language, optional `Translation`, optional `CommunityNote`, canonical URL |
| `Translation` | A machine translation of a tweet's body: `body` (`RichText`), `from` and `to` (BCP 47 codes, optional) |
| `CommunityNote` | Context readers added to a tweet: `body` (`RichText`, its links as spans) and `url`, the note's own page |
| `RichText` | `text: String` + `spans: Vec<Span>`. Spans are **byte ranges into `text`**, sorted and non-overlapping. |
| `SpanKind` | `Mention`, `Hashtag`, `Cashtag`, `Link { display, url }` |
| `Media` | `Image`, `Video`, `Gif`, each with an `ImageRef` that carries size hints (so the list can reserve height) and suggested crops (`Region`s in pixels of the image, one per tile shape; empty when the source has none). A `Video` has its playable files as `VideoVariant`s (URL, bitrate, size), best first; `Media::stream` is the one to play by default |
| `Embed` | `Quote(PostOrGone)`, `LinkCard`, `Poll` |
| `FeedItem` | `Post`, `Repost { by, post }`, `Thread(Vec<Post>)`, `Notification`, `Unavailable`, `Unsupported { reason }` |
| `FeedPage` | `items` + `newer`/`older` page tokens |
| `ListSummary` | A list the viewer owns or follows: `id` (`ListId`), `name`, `members`, `visibility` (`ListVisibility::Public` / `Private`) |
| `FeedSource` | What a column shows: `Home`, `Notifications`, `Search`, `List`, `User`, `Bookmarks`, `Thread`. It's **persisted**. |
| `PostAction` | `Like`, `Unlike`, `Repost`, `Unrepost`, `Bookmark`, `Unbookmark`, … |
| `Draft` | Text, reply target, quote target |
| `RateBudget` | `limit` requests per `window`, as the provider reports for a feed |
| `RequestHealth` | Per request type since startup: ok responses, rows, unsupported rows, warnings, errors, last warning/error (diagnostics) |
| `BackendError` | `Unauthorized`, `RateLimited { reset_at }`, `NotFound`, `Upstream`, `Network`, `Decode` |

The `Backend` trait (`viewer`, `feed`, `thread`, `profile`, `search_users` (one page of accounts matching a query, best match first), `viewer_lists` (the lists the viewer owns or follows, without suggested ones), `act`, `set_following` (a `FollowAction` → the resulting `FollowStatus`: `NotFollowing`, `Following` or `Requested`), `publish`, and `rate_budget` / `health` with empty defaults) is the only way the app reaches a network.

## Translation rules (`twitty-x::translate`)

1. **Never panic and never fail a whole page.** An entry that can't be translated becomes `FeedItem::Unsupported { reason }`, plus a `tracing::warn!` that includes the entry ID and the missing path.
2. **Retweets are unwrapped**: the retweet becomes `FeedItem::Repost { by, post }`, where `post` is the original tweet.
3. **Long tweets**: prefer `note_tweet` text and entities over the truncated `full_text`.
3b. **Translations**: Grok's `grok_translated_post_with_availability` becomes `Post::translation` (its own text and entities, same offset rules) when `is_available` and the text isn't blank; see [x-api.md](x-api.md#grok-translations-2026-10-24).
3c. **Community Notes**: `birdwatch_pivot` becomes `Post::community_note` (the note's text with its links, and its page); a pivot without text or link is dropped with a warning, the tweet stays. See [x-api.md](x-api.md#community-notes-2026-10-24).
4. **Entities**: Twitter gives offsets in **Unicode code points** of the raw, HTML-escaped text (checked against 453 live entities on 2026-10-24: 98 only match as code points, none only as UTF-16). They are converted to byte ranges in the decoded string. Inverted, out-of-range or overlapping entities are dropped with a warning; the text itself is never lost.
4b. **Bios**: a bio's entities only carry `urls` (none of 491 live bios had mentions or hashtags, 2026-10-25), so `translate::text::typed_entities` finds `@mentions`, `#hashtags` and `$cashtags` in the text the way twitter-text does (not after a letter or digit, so e-mail addresses stay plain; not `&#39;`; a hashtag needs a letter). Mentions found this way have no user id and open by handle. Ones that overlap a Twitter link are skipped.
5. **Text clean-up**: decode HTML entities (`&amp;`, `&lt;`, …). Show `t.co` links as their `display_url`, with a `Link` span to the expanded URL. Drop the trailing media `t.co` link, and the leading `@mentions` of replies (both are outside `display_text_range`; reply info is shown separately).
6. **Media**: images and posters use `media_url_https?name=medium`. A video keeps every MP4 variant (HLS playlists are left out), highest bitrate first, with the size Twitter puts in the path (`/vid/avc1/1280x720/…`) when there is one; a GIF's stream is its highest-bitrate MP4. Sizes come from `original_info`. A photo's suggested crops come from `original_info.focus_rects` (Twitter gives five: 16:9, 1:1, about 7:8, 1:2 and the whole image); `ImageRef::with_crops` drops any that are empty or outside the image. Videos and GIFs have none.
6b. **Cards**: `summary`/`summary_large_image`/`player`, `unified_card` (a JSON document inside a binding) and `<id>:broadcast` become `Embed::Link`; `pollNchoice_*` becomes `Embed::Poll`. Unknown cards are ignored, not unsupported. A quote wins over a card.
6c. **Skipped on purpose** (not `Unsupported`): promoted entries, prompts and covers, who-to-follow users, trends and topics. Conversation modules become `FeedItem::Thread` (or a single `Post`).
7. **Unavailable tweets** (`TweetTombstone`, `TweetUnavailable`, withheld, or a deleted quoted tweet) become `Unavailable { reason }` / `PostOrGone::Unavailable`.
8. **Times** become `DateTime<Utc>`. Counts are `u64` and a missing count is `0`, except `Metrics::views`, which is `Option<u64>` because many tweets have no view count and the UI hides it.
9. **Profiles are interned**: the same `UserId` within a page gets the same `Arc<Profile>`.

## Persistence and versioning

Saved files:
- `decks.json`: decks (name, icon), columns, `FeedSource`, and each column's name, width (px) and alerts (sound, volume, system notification). v1 → v2 turned the width presets into pixels (narrow 310, medium 350, wide 400). The deck `icon` was added later without a bump (absent or unknown = the grid icon).
- `settings.json`: theme, `polling` and `animations` (optional, added without a version bump; absent = on), `gif_autoplay` and `sidebar_expanded` (same; absent = off), `media_max_height` (same; pixels, clamped to 150..=800 on load; absent = no limit), `check_updates` (same; absent = on)
- the keyring entry holding the session (`{schema_version, auth_token, ct0}`, see `twitty::account`)
- the optional tweet cache

Rules:
- Each file has a top-level `schema_version: u32`.
- Loading runs migrations `vN → vN+1` until it reaches the current version.
- Each version has a golden file in `crates/twitty/tests/golden/`, and loading it is a test.
- **Never** save Twitter JSON or `PageToken`s that should outlive a session.
- Adding a field: `#[serde(default)]`, with no version bump. Renaming or removing a field or changing its meaning: bump the version and add a migration.
