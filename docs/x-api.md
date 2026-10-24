# X internal API (`twitty-x`)

X doesn't document this API. Everything here comes from community clients and our own captures: [`bun_x`](https://crates.io/crates/bun_x) (Rust, Apache-2.0, the main reference), [x-go](https://github.com/teslashibe/x-go) and [twitter-graphql-scraper](https://github.com/she-llac/twitter-graphql-scraper). Check each claim against live traffic, and update this page whenever behaviour changes.

> Using X's internal API is against X's terms of service. Develop and run live tests with a **secondary account**.

## Endpoint shape

```
GET  https://x.com/i/api/graphql/<queryId>/<OperationName>?variables=<json>&features=<json>&fieldToggles=<json>
POST https://x.com/i/api/graphql/<queryId>/<OperationName>    (mutations, JSON body {variables, features, queryId})
```

Operations planned: `HomeTimeline`, `HomeLatestTimeline`, `TweetDetail`, `UserByScreenName`, `UserTweets`, `UserTweetsAndReplies`, `UserMedia`, `Likes`, `SearchTimeline`, `ListLatestTweetsTimeline`, `Bookmarks`, notifications, `FavoriteTweet`/`UnfavoriteTweet`, `CreateRetweet`/`DeleteRetweet`, `CreateBookmark`/`DeleteBookmark`, `CreateTweet`, `Viewer`.

## Authentication

| Item | Value |
|---|---|
| Cookie `auth_token` | Session token. It's secret, and is stored in keyring. |
| Cookie `ct0` | CSRF token, which must also be sent as the `x-csrf-token` header |
| `authorization` | `Bearer <public web bearer>`, the same for every web user and built into x.com's JS. Keep it as a constant and re-extract it if requests get 403s with an empty body. |
| `x-twitter-auth-type` | `OAuth2Session` |
| `x-twitter-active-user` | `yes` |
| `x-twitter-client-language` | `en` |
| `origin` / `referer` | `https://x.com` / `https://x.com/` |
| `user-agent` | A current stable Chrome UA (update it from time to time) |
| `x-client-uuid`, `x-twitter-client-deviceid` | Stable random UUIDs generated once per install |
| `x-client-transaction-id` | See below |

**Cookie rotation**: X can send new `ct0`/`auth_token` values in `Set-Cookie`. Transport must save them to the session store immediately, or the next request fails.

**Login (v1)**: GPUI's webview doesn't support Windows, so the user copies `auth_token` and `ct0` from their browser's DevTools (Application → Cookies → x.com) into the login dialog. A `Viewer` request checks them before they're saved.

## Query IDs and features

The `queryId` changes with every X web release (about every 2–4 weeks), and `features` changes too. Resolution order:

1. **Runtime cache**: `%LOCALAPPDATA%\Twitty\query-ids.json`, fresh for 24h. A stale cache is still used while a refresh runs in the background.
2. **Refresh**:
   1. Fetch `https://x.com/?lang=en` (and a few other pages).
   2. Regex the `abs.twimg.com/responsive-web/client-web/*.js` bundle URLs out of the HTML.
   3. Fetch the bundles concurrently.
   4. Regex `{queryId:"…",operationName:"…"}`, along with each operation's feature switch list.
3. **Embedded fallback**: `crates/twitty-x/data/operations.json`, a snapshot committed to the repo.

A 404 or a "query not found" error causes **one** forced refresh and retry.

## `x-client-transaction-id`

The real web client builds this from an animation key in the home page plus a hash of method + path + time. For now X accepts a random 32-char hex value per request, and `bun_x` does the same. If X returns **error code 353**, enforcement has arrived. Then port the real algorithm (x-go `transaction.go`, or the `x-client-transaction` crate after checking it against current behaviour) into `transport/txid.rs`.

## Rate limits

- Read `x-rate-limit-limit`, `x-rate-limit-remaining` and `x-rate-limit-reset` (epoch seconds) **for each operation**.
- When `remaining == 0`, don't call again before `reset`, and return `BackendError::RateLimited { reset_at }` straight away. The UI shows a countdown.
- On 429: use `retry-after`/reset if present, otherwise exponential backoff with jitter (1s → 60s cap) and at most 3 retries.
- Column polling intervals adjust to the remaining budget (see [ui.md](ui.md)).

## Response structure (wire)

```
data.<root>                                  # e.g. home.home_timeline_urt, search_by_raw_query.search_timeline
  .timeline.instructions[]
     TimelineAddEntries   { entries[] }
     TimelineReplaceEntry { entry }          # usually refreshes cursors
     TimelinePinEntry     { entry }
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

Promoted entries (`promoted-tweet-*`, or items with `promotedMetadata`) are dropped in `wire`. Who-to-follow and topic modules are skipped in v1.

## Capturing a fixture

1. Run `cargo run -p twitty-x --bin twitty-x-cli -- capture home --out raw.json`, or copy a response from the browser's DevTools.
2. Anonymise it with `cargo run -p twitty-x --bin twitty-x-cli -- anonymise raw.json > tests/fixtures/home/<case>.json`. This replaces handles, names, IDs, text and avatar URLs with deterministic fake values while keeping the structure and UTF-16 offset correctness.
3. Add a test case, run `cargo insta review` and check the model snapshot by eye.

## Runbook: "X broke something"

| Symptom | Likely cause | Action |
|---|---|---|
| 404 on a GraphQL op | Stale queryId | Should self-heal through refresh. If not, fix the bundle regex in `discovery/` and update `data/operations.json`. |
| 400 with "features cannot be null" / feature errors | New feature flag | Refresh the features. If a flag is still missing, add it to `data/operations.json`. |
| Many `Unsupported` items or decode warnings | Schema drift | Capture a fixture, update `wire`/`translate`, and add a snapshot test. **The model must not change unless the new data needs a new model concept.** |
| Error 353 | Transaction ID enforced | Implement the real `txid` algorithm (see above). |
| 401, or error 32/89 | Session expired or revoked | The UI asks the user to log in again. Nothing to fix. |
| 403 with an empty body on everything | Bearer or UA blocked | Re-extract the bearer from `main.*.js` and update the UA. |
| Requests blocked despite correct headers | TLS fingerprinting | Swap the transport client for a Chrome-impersonating one (e.g. `wreq`). This stays inside `transport/`. |

After each incident, add a line to [decisions.md](decisions.md) with the date and what changed.
