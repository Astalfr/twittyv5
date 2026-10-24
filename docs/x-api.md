# X internal API (`twitty-x`)

X doesn't document this API. Everything here comes from community clients and our own observations: [`bun_x`](https://crates.io/crates/bun_x) (Rust, Apache-2.0, the main reference), [x-go](https://github.com/teslashibe/x-go) and [twitter-graphql-scraper](https://github.com/she-llac/twitter-graphql-scraper). Check each claim against live traffic, and update this page whenever behaviour changes.

> Using X's internal API is against X's terms of service. Develop and run live tests with a **secondary account**.

**M10:** `XBackend` keeps per-operation health counters (`Backend::health`), shown in Settings → Diagnostics; a response with no timeline at all counts as a warning. Requests carry a real transaction id since 2026-10-24 (see below).

**M8:** `XBackend::act` and `publish` send the web client's mutations (below). They are tested against a mock server only; the first real use is the user's.

**M7:** every `FeedSource` is mapped to a live-verified operation (user replies, media and likes, and lists added on 2026-10-24). Media grids page through `TimelineAddToModule`.

**M5:** `XBackend` implements `twitty-model`'s `Backend` and the app uses it when it finds a session. Reads (`viewer`, `feed`, `thread`, `profile`) are wired; writes (`act`, `publish`) return an error until M8. Verified live on 2026-10-24: Home, Notifications and Search columns load with images.

**M4:** X responses are translated into `twitty-model` by `translate::feed_page` and checked against 9 anonymised live fixtures.

**M3:** transport, catalog, discovery, `XClient` and the dev CLI are implemented, tested against a mock server, and **verified live on 2026-10-24**: `HomeTimeline` and `HomeLatestTimeline` returned data with the declared feature subsets, and logged-in discovery found every operation Twitty uses. Translation into `twitty-model` (`wire` + `translate`) is M4.

## Code map

| Module | Role |
|---|---|
| `session.rs` | `Session { auth_token, ct0 }` built from `Secret`s. `Debug` is redacted. Loaded from env vars or a JSON file. |
| `transport/` | `Transport`: static + per-request headers, cookie rotation (`on_rotate` hook), per-operation rate limits, retries, HTTP/X-error mapping. Knows nothing about GraphQL shapes. |
| `catalog.rs` | `Catalog`: resolves an operation name to an `OperationSpec` (query id, method, feature switches, field toggles), builds the `features` map. |
| `discovery.rs` | Best-effort query-id discovery from x.com's bundles (legacy + Relay formats). |
| `client.rs` | `XClient::graphql(op, variables)` and helpers like `home_timeline`. Recovers once from stale ids. `XClient::production` builds one with the default cache paths and discovery (used by the app and the CLI). |
| `requests.rs` | Private: which operation and variables serve each `FeedSource` and `PageRequest`. Only live-verified variable sets are mapped. |
| `backend.rs` | `XBackend`: `Backend` over `XClient` + `translate`. |
| `wire.rs` | Private, lenient readers over X JSON: `at`/`str_at`/`u64_at`…, `timeline()` classifying entries (items, modules, cursors). |
| `translate/` | X JSON → `twitty-model`: `text` (entities → `RichText`), `user`, `tweet`, `media`, `card`, `notification`; entry points `feed_page()` (any timeline), `thread()` (`TweetDetail` split around the focal post), `viewer()` and `user()`. |
| `anonymise.rs` | Scrubs captures into committable fixtures, plus a leak check. |
| `bin/twitty-x-cli.rs` | Dev tool: `ops`, `refresh-ids`, `capture home`, `capture op`, `anonymise`. |
| `data/operations.json` | Embedded fallback ids with feature switches and field toggles, regenerated from a live discovery run. |
| `data/default_features.json` | Default `features` values (from `bun_x`, Apache-2.0). |

## Endpoint shape

```
GET  https://x.com/i/api/graphql/<queryId>/<OperationName>?variables=<json>&features=<json>[&fieldToggles=<json>]
POST https://x.com/i/api/graphql/<queryId>/<OperationName>    (mutations, JSON body {variables, features, queryId[, fieldToggles]})
POST https://x.com/i/api/1.1/<path>.json                       (the few v1.1 REST actions, form body; see Follow below)
```

Confirmed live on 2026-10-24: the logged-in web client uses this old-style `<id>/<OperationName>` path (e.g. `0toR0MxC06yY3Fc57GXBpA/HomeTimeline`) with `features` and `fieldToggles` in the query string.

## Authentication

| Item | Value |
|---|---|
| Cookie `auth_token` | Session token. Secret. Stored in Windows Credential Manager by the sign-in dialog (see "Signing in"). |
| Cookie `ct0` | CSRF token, also sent as the `x-csrf-token` header |
| `authorization` | `Bearer <public web bearer>` (`transport::WEB_BEARER`), the same for every web user. Re-extract it if requests get 403s with an empty body. |
| `x-twitter-auth-type` | `OAuth2Session` |
| `x-twitter-active-user` | `yes` |
| `x-twitter-client-language` | `en` |
| `origin` / `referer` | `https://x.com` / `https://x.com/` |
| `user-agent` | A current stable Chrome UA (`transport::CHROME_UA`, update from time to time) |
| `x-client-uuid`, `x-twitter-client-deviceid` | `ClientIds`: random UUIDs, saved once per install (`%LOCALAPPDATA%\Twitty\cache\client-ids.json`) |
| `x-client-transaction-id` | Signed per request from the home page's key (`transport/transaction.rs`). See below. |

**Cookie rotation**: when a response's `Set-Cookie` carries a new `ct0`/`auth_token`, the transport switches to it for the next request and calls the `on_rotate` hook. The app writes the rotated session back to where it came from: the keyring, or the session file (write + rename); sessions from env vars aren't persisted. Empty values (deletions) are ignored; an expired session shows up as 401 instead.

**Login (v1)**: GPUI's webview doesn't support Windows, so the user copies `auth_token` and `ct0` from their browser's DevTools (Application → Cookies → x.com).

### Signing in

Use **Sign in** (banner or rail). The dialog explains where to find the cookies, normalises what was pasted (quotes, `auth_token=` prefixes, spaces), validates it with `Session::new`, then calls `Viewer`. Only a session X accepts is stored (Windows Credential Manager, entry `Twitty` / `x-session`).

At startup the app takes the first of:

1. `TWITTY_AUTH_TOKEN` and `TWITTY_CT0` environment variables;
2. the keyring entry stored by the dialog;
3. `%APPDATA%\Twitty\config\session.json`, then `session.json` in the working directory (for development; gitignored).

A session file is `{"auth_token": "…", "ct0": "…"}`. Unusable sources are logged (never their contents) and skipped. With none of these, the app runs on demo data with a banner. At startup the app calls `Viewer`: on success the rail's account button shows `@handle`; on `Unauthorized` the sign-in dialog opens with "session expired" (the stored entry is kept until a new sign-in replaces it).

**Sign out** (account button) deletes the keyring entry and returns to demo data. Sessions from env vars or files can't be signed out from the app: the next launch would use them again, so the account panel says where the session comes from instead.

### What each column calls

| `FeedSource` | Operation | Notes |
|---|---|---|
| `Home { ForYou }` / `Home { Following }` | `HomeTimeline` / `HomeLatestTimeline` | |
| `Notifications { filter }` | `NotificationsTimeline` | `timeline_type`: `All`, `Verified`, `Mentions` |
| `Search { query, sort }` | `SearchTimeline` (POST) | `product`: `Top`, `Latest`, `Media` |
| `User { tab: Posts }` | `UserTweets` | |
| `User { tab: Replies }` | `UserTweetsAndReplies` | |
| `User { tab: Media }` | `UserMedia` | page 1 is a `VerticalGrid` module; later pages come as `TimelineAddToModule` |
| `User { tab: Likes }` | `Likes` | X only shows the signed-in account's likes; other accounts come back empty |
| `List { id }` | `ListLatestTweetsTimeline` | |
| `Bookmarks` | `Bookmarks` | |
| `Thread { root }` | `TweetDetail` | |

`PageRequest::Older` sends the bottom cursor and `Newer` the top cursor, both as `cursor`. A page with no rows ends the feed even if X sends a cursor, so scrolling can't loop on empty pages.

## Query IDs and features

The `queryId` changes with X web releases (about every 2–4 weeks), and `features` changes too. `Catalog::resolve` checks, in order:

1. **Override**: set with `--query-id` in the CLI or `Catalog::set_override`. Never auto-refreshed.
2. **Runtime cache**: `%LOCALAPPDATA%\Twitty\cache\query-ids.json`, written by discovery, fresh for 24h. Names are looked up as-is, then with a `Query` suffix (Relay naming).
3. **Embedded fallback**: `data/operations.json`.

On HTTP 404, `XClient::graphql` tries two things, in order:
1. **Wrong method:** some queries are only served over POST (`SearchTimeline` returned 404 to GET with a valid id on 2026-10-24). A GET 404 is retried once as POST (JSON body with `variables`, `features`, `queryId`); on success POST is remembered for that operation.
2. **Stale id:** discovery runs **once** (logged in) and the call is retried **once** if a different id was found. Otherwise it returns `XError::StaleQueryId { operation, query_id }`.

`features`: when the operation's feature switch names are known (legacy bundles declare them), only those are sent, with values from `default_features.json` (unknown names default to `true`). When they're unknown, the whole default map (~150 flags) is sent.

### Two web clients (finding from 2026-10-24)

x.com now serves two different web clients:

| | Legacy (`responsive-web/client-web/*.js`) | New (`x-web/x-web/*.js`, Vite + Relay) |
|---|---|---|
| Seen on | The logged-in app (`https://x.com/home` with a session): `main`, `vendor` and a locale script, plus ~650 lazily loaded webpack chunks | The logged-out site (`https://x.com/?lang=en`) |
| Operation declaration | `{queryId:"…",operationName:"HomeTimeline",operationType:"query",metadata:{featureSwitches:[…],fieldToggles:[…]}}` | `params:{id:"…",metadata:{},name:"HomeLatestTimelineQuery",operationKind:"query",text:null}` (Relay persisted query) |
| Feature switches in bundle | Yes | No |
| Bundle list | Script tags of the page, plus the **inline webpack chunk maps** in the HTML: `{id:"bundle.HomeTimeline",…}` and `{id:"<16 hex>",…}`; a chunk's URL is `<dir of main.js>/<name>.<hash>a.js` | Entry script, then lazily loaded `assets/*.js` chunks (paths relative to the bundle base) |

The service worker (`sw.js`) and the logged-out HTML no longer list `responsive-web` bundles, so discovery methods that rely on them find nothing on the logged-out site.

What the live runs showed:
- **Logged out:** 204 Relay operations in 250 `x-web` scripts in ~1.5s.
- **Logged in:** the legacy client. Its `main.js` declares ~100 operations; the rest (including `HomeTimeline`, `HomeLatestTimeline`, `Bookmarks`, `NotificationsTimeline`, `ListLatestTweetsTimeline`) are only in lazily loaded chunks reachable through the inline chunk maps. The logged-in chunks also contain Relay persisted queries. One run: 464 scripts, 585 legacy + 465 Relay declarations, ~6s.
- 11 of the 18 ids `bun_x` published the day before were already stale (`HomeLatestTimeline`, `TweetDetail`, `SearchTimeline`, `UserByScreenName`, `UserTweets`, `UserTweetsAndReplies`, `UserMedia`, `Likes`, `ListLatestTweetsTimeline`, `Viewer`, `CreateTweet`); the like/repost/bookmark mutations were unchanged.
- Twitty calls legacy-named operations. Relay operations also work on the same endpoint, as `graphql/<id>/<relayName>` with `{variables, features, queryId}` (verified live 2026-10-24 with `retweetButtonRetweetMutation`). Their variables are camelCase (`tweetId`), and a wrong name gets HTTP 422 `GRAPHQL_VALIDATION_FAILED`.

### Discovery algorithm

1. Fetch `https://x.com/home` and `https://x.com/?lang=en`, with the session cookie when there is one.
2. Collect `<script src>` URLs, any `https://abs.twimg.com/…js` URL, and the legacy webpack chunks from the inline chunk maps (relevant names first; locales skipped).
3. Download scripts 8 at a time (at most 600 per run). Extract operations in both formats.
4. Follow `assets/<name>.js` references whose name looks relevant (`graphql`, `timeline`, `home`, `tweet`, `search`, `bookmark`, `notif`, `list`, `profile`, `user`, …), resolved against the bundle base.
5. Stop early once every operation of the embedded catalog was found (checked after each batch).
6. Merge the results into the cache file.

## `x-client-transaction-id`

X enforces this header on some writes. With a random value, `CreateRetweet` and `CreateBookmark` get **HTTP 404**, which looks exactly like a stale query id: rediscovery finds the same id. A Relay mutation got error **226** ("looks like it might be automated") on a real post, probably for the same reason; 226 can also be an account-level signal that no header fixes. Likes, unbookmarking and reads still passed with random ids on 2026-10-24. So a 404 on a write whose id refresh reports `changed=0` points here, not at the catalog.

`transport/transaction.rs` ports the web client's algorithm (after XClientTransaction, MIT):
1. **Key material**, loaded once per hour by `Signer` and fetched logged in, with its own plain HTTP client. It comes from `https://x.com/home`:
   - the base64 `content` of `<meta name="twitter-site-verification">` (the key, 48 bytes);
   - the four `<svg id="loading-x-anim-N">`. Frame `key[5] % 4` is used, and its second `<path d>` after `M 10,30 C` is split on `C` into 16 rows of 11 numbers.
   - the `ondemand.s.<hash>a.js` chunk, found through the inline webpack chunk maps (`discovery::extract_webpack_chunks`). Every `(x[N],16)` in it is an index into the key: the first picks the row (`key[i] % 16`), and the rest multiply into the frame time (`Π key[i] % 16`, rounded to tens, over 4096 ms).
2. **Animation key**: the row is evaluated at that time. A cubic-bezier easing from row[7..11] interpolates colour row[0..3] → row[3..6] and a rotation of 0 → row[6] scaled to 60–360°. The result is printed as hex: rounded colour, then the |values| of the rotation matrix rounded to 2 decimals, as hex fractions.
3. **Per request**: `t = (now_ms − 1682924400000) / 1000`, and `sha256("{METHOD}!{/i/api/… path}!{t}obfiowerehiring{animation key}")`. Then `[r, (key ++ t as 4 LE bytes ++ hash[..16] ++ [3]) XOR r]` for a random byte `r`, in base64 without padding (94 chars).

If the page can't be read or parsed, requests fall back to a random id with a warning, and loading is retried after 5 minutes. A request never fails because of the signer. Tests don't enable it (`Transport::signing_from` is opt-in; `XClient::production` turns it on). They use a synthetic page and chunk in `tests/fixtures/transaction/`, with vectors computed by a straight Python port of the reference.

When writes start failing again with 404, 226 or 353, check that `twitty-x-cli capture op CreateRetweet --vars '{"tweet_id":"1","dark_request":false}' --out dumps/probe.json` still answers error 144 (no such tweet), which means it was signed and accepted. If not, compare the page markers above with x.com's current HTML and chunk.

## Rate limits and retries

- `x-rate-limit-limit`, `x-rate-limit-remaining` and `x-rate-limit-reset` (epoch seconds) are recorded **per operation** (`RateLimits`).
- When `remaining == 0` and the reset is in the future, calls fail locally with `XError::RateLimited { reset_at }` without hitting X.
- HTTP 429 → `RateLimited` with the reset from the headers (or now + 1 min).
- Network errors and 5xx are retried with exponential backoff (default 3 retries, 1s base). 401/403/404/429 are never retried.
- Column polling will adjust to the remaining budget (see [ui.md](ui.md)).

## Error mapping

| Response | `XError` | `BackendError` |
|---|---|---|
| 200 with `data` (with or without `errors`) | Ok (partial errors are logged) | – |
| 200 with only `errors`, code 32/89/215 | `Unauthorized` | `Unauthorized` |
| … code 88 | `RateLimited` | `RateLimited` |
| … code 353 | `TransactionIdRequired` | `Upstream` |
| … code 34/144 | `Api` | `NotFound` |
| 401, 403 | `Unauthorized` (403 + code 353 → `TransactionIdRequired`) | `Unauthorized` |
| 403 + code 158/160/161/162 (follow refused: yourself, already requested, follow limit, blocked) | `Api` (`transport::REFUSED`) | `Upstream`, except 160 → `FollowStatus::Requested` |
| 404 | refresh + retry, then `StaleQueryId` | `Upstream` |
| 429 | `RateLimited` | `RateLimited` |
| 5xx (after retries), other | `Http { status, snippet }` | `Upstream` |
| connection / TLS / timeout | `Network` | `Network` |

## Mutations (M8)

| Action | Operation | Variables |
|---|---|---|
| Like / unlike | `FavoriteTweet` / `UnfavoriteTweet` | `{"tweet_id"}` |
| Repost / undo | `CreateRetweet` / `DeleteRetweet` | `{"tweet_id", "dark_request": false}` / `{"source_tweet_id", "dark_request": false}` |
| Bookmark / remove | `CreateBookmark` / `DeleteBookmark` | `{"tweet_id"}` |
| Post / reply | `CreateTweet` | `{"tweet_text", "dark_request": false, "media": {"media_entities": [], "possibly_sensitive": false}, "semantic_annotation_ids": [], "disallowed_reply_options": null}` plus `reply: {"in_reply_to_tweet_id", "exclude_reply_user_ids": []}` for replies, `attachment_url: "https://x.com/i/status/<id>"` for quotes |

All are POSTs with `{variables, features, queryId}`. Error handling (`backend.rs`):
- codes **139** ("already favorited") and **327** ("already retweeted") count as success, so a double click or a stale view doesn't roll back a state that is in fact right;
- any other `errors` with empty `data` becomes `BackendError::Upstream("X error <code>: <message>")`, which the UI shows (rollback for toggles, error text in compose);
- `CreateTweet` returns the new tweet in `data.create_tweet.tweet_results.result`, translated with `translate::created_post`.

### Follow / unfollow

The web client has no GraphQL mutation for this (the catalog has none): it POSTs to v1.1 REST, `1.1/friendships/create.json` / `1.1/friendships/destroy.json`, with a form body (`application/x-www-form-urlencoded`) of `user_id` plus the `include_*`/`skip_status=1` flags it sends (`requests::follow`). They go through `XClient::rest_post` → `transport::Body::Form`, with the same cookies, csrf and transaction id, but without query-id refresh or GET→POST fallback. The reply is the account as a v1.1 user object; `translate::follow_status` reads it: an unfollow is `NotFollowing`; a follow is `Requested` when `follow_request_sent` is true (or the account is `protected` and not `following`), else `Following`. Its `following` can still show the state before the call, so a plain `false` isn't trusted. Fixtures `friendships-create*.json` are hand-written (no real account), not captures.

**Not verified live**: these shapes come from the web client and community clients. If a write fails with a new error code, capture it (the message is in the UI and the log) and record it here.

## Response structure (wire, M4)

```
data.<root>                                  # e.g. home.home_timeline_urt, search_by_raw_query.search_timeline
  .timeline.instructions[]
     TimelineAddEntries   { entries[] }
     TimelineReplaceEntry { entry }          # usually refreshes cursors
     TimelinePinEntry     { entry }
     TimelineAddToModule  { moduleEntryId, moduleItems[] }   # later pages of a module (media grid)
     TimelineClearCache / TimelineTerminateTimeline / TimelineShowAlert ...
entry.content.__typename / entryType
     TimelineTimelineItem    -> itemContent.tweet_results.result
     TimelineTimelineModule  -> items[].item.itemContent... (conversation / who-to-follow)
     TimelineTimelineCursor  -> cursorType Top|Bottom, value
result.__typename
     Tweet | TweetWithVisibilityResults (.tweet) | TweetTombstone | TweetUnavailable
tweet: rest_id, core.user_results.result, legacy{full_text, entities, extended_entities, ...},
       note_tweet.note_tweet_results.result, quoted_status_result, legacy.retweeted_status_result,
       card, views.count
```

Promoted entries (`promoted-tweet-*`, or items with `promotedMetadata`) will be dropped in `wire`. Who-to-follow and topic modules are skipped in v1. Tombstones get their `FeedItem` row key from the timeline `entryId`.

Observed live on 2026-10-24 (input for M4):
- `HomeTimeline`: one `TimelineAddEntries` with `cursor-top`, 28 `tweet-*`, 7 `promoted-tweet-*`, `cursor-bottom`; all results were `Tweet`.
- **User objects changed**: `core.user_results.result` has no `legacy` block anymore. Name and handle are in `core.{name, screen_name, created_at}`, and there are `avatar`, `banner`, `profile_bio`, `location`, `website`, `verification`, `is_blue_verified`, `relationship_counts`, `relationship_perspectives`, `tweet_counts`, `privacy`.
- Tweets still have `legacy` (`full_text`, `entities`, `extended_entities`, counts, `favorited`, `retweeted`, `bookmarked`, `created_at`, `quoted_status_id_str`), plus `views`, `quoted_status_result`, `edit_control`, `grok_*`.
- `HomeLatestTimeline` on the test account returned no posts: a `TimelineShowCover` (email review prompt) and an entry `flip-prompt-message-grok-bot-inline`. Covers and prompts must be skipped.

## Dev CLI

The session comes from `--session <file>` (`{"auth_token":"…","ct0":"…"}`; `session*.json` at the repo root is gitignored) or the `TWITTY_AUTH_TOKEN` / `TWITTY_CT0` env vars. Values are never printed.

```sh
cargo run -p twitty-x --bin twitty-x-cli -- ops                        # catalog: id, method, origin
cargo run -p twitty-x --bin twitty-x-cli -- --session session.json refresh-ids
cargo run -p twitty-x --bin twitty-x-cli -- --session session.json capture home [--latest] [--count 20] [--cursor C] [--query-id ID] [--out dumps/home.json]
cargo run -p twitty-x --bin twitty-x-cli -- --session session.json capture op TweetDetail --vars '{"focalTweetId":"…"}' --out dumps/tweetdetail.json
cargo run -p twitty-x --bin twitty-x-cli -- anonymise dumps/home.json crates/twitty-x/tests/fixtures/home.json [--allow word,…]
```

Variables that worked live (2026-10-24):

| Operation | Variables |
|---|---|
| `UserByScreenName` | `{"screen_name":"NASA"}` |
| `UserTweets` | `{"userId":"…","count":40,"includePromotedContent":true,"withQuickPromoteEligibilityTweetFields":true,"withVoice":true}` |
| `SearchTimeline` (POST) | `{"rawQuery":"rust lang:en","count":20,"querySource":"typed_query","product":"Top"}` (`Latest`, `Top`, `Media`) |
| `TweetDetail` | `{"focalTweetId":"…","with_rux_injections":false,"rankingMode":"Relevance","includePromotedContent":true,"withCommunity":true,"withQuickPromoteEligibilityTweetFields":true,"withBirdwatchNotes":true,"withVoice":true}` |
| `Bookmarks` | `{"count":20,"includePromotedContent":true}` |
| `NotificationsTimeline` | `{"timeline_type":"All","count":20}` |
| `UserTweetsAndReplies` | `{"userId":"…","count":20,"includePromotedContent":true,"withCommunity":true,"withVoice":true}` |
| `UserMedia`, `Likes` | `{"userId":"…","count":20,"includePromotedContent":false,"withClientEventToken":false,"withBirdwatchNotes":false,"withVoice":true}` |
| `ListLatestTweetsTimeline` | `{"listId":"…","count":20}` |
| `SearchTimeline` with `"product":"Lists"` | finds public lists (`TimelineTwitterList` items; used to find a list id for the fixture) |

`capture` writes the raw response (to the gitignored `dumps/` by default) and prints the instruction types, entry count and rate-limit budget.

## Live tests (opt-in, never in CI)

```sh
# Extractors over downloaded bundles (not committed):
TWITTY_BUNDLE_DIR=<dir> cargo test -p twitty-x --test live -- --ignored bundles --nocapture
# Logged-out discovery crawl of the public site:
TWITTY_LIVE=1 cargo test -p twitty-x --test live -- --ignored discovery_logged_out --nocapture
# Real HomeTimeline call (secondary account):
TWITTY_LIVE=1 TWITTY_AUTH_TOKEN=… TWITTY_CT0=… cargo test -p twitty-x --test live -- --ignored home
```

## Capturing a fixture

1. Capture into the gitignored `dumps/`: `twitty-x-cli capture home` / `capture op …`.
2. Scrub it: `twitty-x-cli anonymise dumps/x.json crates/twitty-x/tests/fixtures/x.json`.
   - Keeps the JSON structure, all keys and numbers, dates, `__typename`s, card type names, X's own hosts and URL route words, and the code-point length of every string (so entity offsets stay valid).
   - Replaces text, names, handles, ids, other hosts, URL paths and opaque tokens with pseudo-random characters. A word or id maps to the same replacement everywhere in the file (handle in `screen_name` = `@handle` in text, `id_str` = id in `entryId`), but the key is random per run, so it can't be reversed with a dictionary.
   - Refuses to write if any original handle or user display name survives in a string value. Reviewed false positives (e.g. an account literally named "None" colliding with enum values) can be passed with `--allow`.
3. Add the name to `FIXTURES` in `tests/translate.rs`, run `cargo insta review` and read the summary snapshot.

Never commit a raw capture: it contains other people's posts and profile data. Polls and tombstones didn't appear in live captures yet; they are covered by synthetic unit tests in `translate/card.rs` and `translate/tweet.rs`.

## Runbook: "X broke something"

| Symptom | Likely cause | Action |
|---|---|---|
| 404 on a write whose refresh reports `changed=0`, or error 226 | Transaction id rejected | See `x-client-transaction-id` above. |
| 404 on a GraphQL op | Stale queryId | Should self-heal through discovery. If not, run `twitty-x-cli refresh-ids` and look at the report. If no operations were found, the bundle format changed: update the extractors in `discovery.rs` (and their unit tests), then update `data/operations.json`. |
| 400 with "features cannot be null" / feature errors | New feature flag | Refresh ids (legacy bundles declare switches). If a flag is still missing, add it to `data/default_features.json`. |
| Many `Unsupported` items or decode warnings (Settings → Diagnostics shows them per operation, with the last reason; also in the log) | Schema drift | Capture a fixture, update `wire`/`translate`, and add a snapshot test. **The model must not change unless the new data needs a new model concept.** |
| Error 353 | Transaction ID rejected | The page markers or the algorithm changed (see above). |
| 401, or error 32/89 | Session expired or revoked | The UI asks the user to log in again. Nothing to fix. |
| 403 with an empty body on everything | Bearer or UA blocked | Re-extract the bearer from the bundles and update `CHROME_UA`. |
| Requests blocked despite correct headers | TLS fingerprinting | Swap the transport client for a Chrome-impersonating one (e.g. `wreq`). This stays inside `transport/`. |

After each incident, add a line to [decisions.md](decisions.md) with the date and what changed.
