//! Tweet and bio text: Twitter's raw text + entity list → `RichText`.
//!
//! Twitter gives entity offsets in **Unicode code points** of the raw (HTML-escaped)
//! text, verified against live data on 2026-10-24. `display_text_range`
//! (also code points) cuts the leading reply mentions and the trailing media
//! link. Output spans are byte ranges into the decoded text.

use serde_json::Value;
use twitty_model::{RichText, Span, SpanKind, UserId, UserRef};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
use url::Url;

use crate::wire::{array_at, at, str_at};

/// An entity as Twitter describes it, before translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawEntity {
    /// Code-point offsets into the raw text.
    pub start: usize,
    pub end: usize,
    pub kind: RawKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawKind {
    Mention {
        handle: String,
        id: Option<String>,
    },
    Hashtag(String),
    Cashtag(String),
    /// A `t.co` link: shown as `display`, opening `expanded`.
    Url {
        display: String,
        expanded: String,
    },
    /// The trailing `t.co` link Twitter appends for attached media: removed.
    Media,
}

fn indices(e: &Value) -> Option<(usize, usize)> {
    let i = array_at(e, &["indices"]);
    let a = usize::try_from(i.first()?.as_u64()?).ok()?;
    let b = usize::try_from(i.get(1)?.as_u64()?).ok()?;
    (a < b).then_some((a, b))
}

/// Reads a Twitter entity object (`entities`, `entity_set`, bio `description`).
pub(crate) fn entities(set: &Value) -> Vec<RawEntity> {
    let mut out = Vec::new();
    let mut push = |key: &str, kind: &dyn Fn(&Value) -> Option<RawKind>| {
        for e in array_at(set, &[key]) {
            if let (Some((start, end)), Some(kind)) = (indices(e), kind(e)) {
                out.push(RawEntity { start, end, kind });
            }
        }
    };
    push("user_mentions", &|e| {
        Some(RawKind::Mention {
            handle: str_at(e, &["screen_name"])?.to_owned(),
            id: str_at(e, &["id_str"]).map(str::to_owned),
        })
    });
    push("hashtags", &|e| Some(RawKind::Hashtag(str_at(e, &["text"])?.to_owned())));
    push("symbols", &|e| Some(RawKind::Cashtag(str_at(e, &["text"])?.to_owned())));
    push("urls", &|e| {
        let expanded = str_at(e, &["expanded_url"]).or_else(|| str_at(e, &["url"]))?;
        let display = str_at(e, &["display_url"]).unwrap_or(expanded);
        Some(RawKind::Url { display: display.to_owned(), expanded: expanded.to_owned() })
    });
    push("media", &|_| Some(RawKind::Media));
    out
}

/// The mentions, hashtags and cashtags typed in `raw`, found the way Twitter's
/// web client finds them (a rough port of twitter-text). Bios need this: their
/// entities only carry `urls` (checked on 491 live bios, 2026-10-25). Offsets
/// are code points, like Twitter's.
pub(crate) fn typed_entities(raw: &str) -> Vec<RawEntity> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let before = i.checked_sub(1).and_then(|j| chars.get(j).copied());
        let found = match chars[i] {
            '@' | '＠' => mention(&chars, i, before),
            '#' | '＃' => hashtag(&chars, i, before),
            '$' => cashtag(&chars, i, before),
            _ => None,
        };
        match found {
            Some(e) => {
                i = e.end;
                out.push(e);
            }
            None => i += 1,
        }
    }
    out
}

fn handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Letters, digits, `_` and combining marks (Hindi, Thai… hashtags).
fn hashtag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c.general_category_group() == GeneralCategoryGroup::Mark
}

/// The end of the run of `ok` characters starting at `from`.
fn run_end(chars: &[char], from: usize, ok: impl Fn(char) -> bool) -> usize {
    chars
        .get(from..)
        .and_then(|rest| rest.iter().position(|&c| !ok(c)))
        .map_or(chars.len(), |n| from + n)
}

/// Whether the text at `end` goes on like an e-mail address or a URL, which
/// makes what came before it not an entity.
fn goes_on_as_address(chars: &[char], end: usize) -> bool {
    matches!(chars.get(end), Some('@' | '＠' | '#' | '＃'))
        || chars.get(end..end + 3) == Some(&[':', '/', '/'][..])
}

fn mention(chars: &[char], at: usize, before: Option<char>) -> Option<RawEntity> {
    // `name@example.com` is an e-mail address, not a mention.
    if before.is_some_and(|c| handle_char(c) || "@＠!#$%&*".contains(c)) {
        return None;
    }
    let end = run_end(chars, at + 1, handle_char);
    if !(1..=15).contains(&(end - at - 1)) || goes_on_as_address(chars, end) {
        return None;
    }
    let handle = chars[at + 1..end].iter().collect();
    Some(RawEntity { start: at, end, kind: RawKind::Mention { handle, id: None } })
}

fn hashtag(chars: &[char], at: usize, before: Option<char>) -> Option<RawEntity> {
    // The raw text is HTML-escaped: `&#39;` is not a hashtag.
    if before.is_some_and(|c| hashtag_char(c) || c == '&') {
        return None;
    }
    let end = run_end(chars, at + 1, hashtag_char);
    let tag = &chars[at + 1..end];
    // At least one letter: `#123` and the keycap `#️⃣` (marks only) are not tags.
    if !tag.iter().any(|c| c.is_alphabetic()) || goes_on_as_address(chars, end) {
        return None;
    }
    Some(RawEntity { start: at, end, kind: RawKind::Hashtag(tag.iter().collect()) })
}

/// `$TSLA`, `$BRK.A`: 1 to 6 letters, then maybe a `.` or `_` and 1 or 2 more.
fn cashtag(chars: &[char], at: usize, before: Option<char>) -> Option<RawEntity> {
    if before.is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let letters = |from| run_end(chars, from, |c: char| c.is_ascii_alphabetic());
    let mut end = letters(at + 1);
    if !(1..=6).contains(&(end - at - 1)) {
        return None;
    }
    if matches!(chars.get(end), Some('.' | '_')) {
        let suffix = letters(end + 1);
        if (1..=2).contains(&(suffix - end - 1)) {
            end = suffix;
        }
    }
    if chars.get(end).is_some_and(|&c| hashtag_char(c)) || goes_on_as_address(chars, end) {
        return None;
    }
    Some(RawEntity { start: at, end, kind: RawKind::Cashtag(chars[at + 1..end].iter().collect()) })
}

/// Adds the entities typed in `raw` to Twitter's own, skipping those that
/// overlap one of Twitter's (an `@` inside a link).
pub(crate) fn with_typed_entities(raw: &str, mut ents: Vec<RawEntity>) -> Vec<RawEntity> {
    let typed: Vec<_> = typed_entities(raw)
        .into_iter()
        .filter(|t| !ents.iter().any(|e| e.start < t.end && t.start < e.end))
        .collect();
    ents.extend(typed);
    ents
}

/// Reads the entities of Twitter's "rich message" texts (Community Notes, …):
/// `{fromIndex, toIndex, ref: {type, url, expandedUrl}}`, with code-point
/// offsets into `raw`. Only links are kept; their covered text is already the
/// display form, so it stays as is.
pub(crate) fn rich_message_entities(raw: &str, ents: &[Value]) -> Vec<RawEntity> {
    let chars: Vec<char> = raw.chars().collect();
    let index = |e: &Value, k: &str| usize::try_from(at(e, &[k])?.as_u64()?).ok();
    ents.iter()
        .filter_map(|e| {
            let (start, end) = (index(e, "fromIndex")?, index(e, "toIndex")?);
            let display: String = chars.get(start..end).filter(|c| !c.is_empty())?.iter().collect();
            if str_at(e, &["ref", "type"]) != Some("TimelineUrl") {
                tracing::debug!(?e, "rich message entity that isn't a link dropped");
                return None;
            }
            let expanded =
                str_at(e, &["ref", "expandedUrl"]).or_else(|| str_at(e, &["ref", "url"]))?;
            Some(RawEntity {
                start,
                end,
                kind: RawKind::Url { display, expanded: expanded.to_owned() },
            })
        })
        .collect()
}

/// Builds rich text from raw text, entities and an optional display range.
///
/// Entities outside the display range, overlapping an earlier entity, or with
/// out-of-range offsets are dropped with a warning; text is never lost.
pub(crate) fn rich_text(
    raw: &str,
    mut ents: Vec<RawEntity>,
    range: Option<(usize, usize)>,
) -> RichText {
    let chars: Vec<char> = raw.chars().collect();
    let (start, end) = match range {
        Some((a, b)) if a <= b => (a.min(chars.len()), b.min(chars.len())),
        _ => (0, chars.len()),
    };
    ents.sort_by_key(|e| e.start);

    let mut text = String::with_capacity(raw.len());
    let mut spans = Vec::new();
    let mut cursor = start;
    for e in ents {
        if e.start >= e.end || e.start < start || e.end > end {
            // Reply-prefix mentions and the trailing media link live outside the range.
            if !matches!(e.kind, RawKind::Media | RawKind::Mention { .. }) || e.end > chars.len() {
                tracing::debug!(?e, "entity outside display range dropped");
            }
            continue;
        }
        if e.start < cursor {
            tracing::warn!(?e, "overlapping entity dropped");
            continue;
        }
        text.push_str(&decode(&chars[cursor..e.start]));
        let original: String = chars[e.start..e.end].iter().collect();
        let shown = match &e.kind {
            RawKind::Media => {
                cursor = e.end;
                continue;
            }
            RawKind::Url { display, .. } => display.clone(),
            _ => html_escape::decode_html_entities(&original).into_owned(),
        };
        let from = text.len();
        text.push_str(&shown);
        if let Some(kind) = span_kind(&e.kind) {
            spans.push(Span { range: from..text.len(), kind });
        }
        cursor = e.end;
    }
    text.push_str(&decode(&chars[cursor..end]));

    // A removed media link often leaves trailing whitespace.
    let keep = spans.last().map_or(0, |s| s.range.end);
    let trimmed = text.trim_end().len().max(keep);
    text.truncate(trimmed);

    RichText::new(text.clone(), spans).unwrap_or_else(|err| {
        tracing::warn!("invalid spans ({err}); keeping plain text");
        RichText::plain(text)
    })
}

fn decode(chars: &[char]) -> String {
    let s: String = chars.iter().collect();
    html_escape::decode_html_entities(&s).into_owned()
}

fn span_kind(kind: &RawKind) -> Option<SpanKind> {
    Some(match kind {
        RawKind::Mention { handle, id } => SpanKind::Mention(UserRef {
            id: id.as_deref().map(UserId::new),
            handle: handle.as_str().into(),
        }),
        RawKind::Hashtag(tag) => SpanKind::Hashtag { tag: tag.as_str().into() },
        RawKind::Cashtag(symbol) => SpanKind::Cashtag { symbol: symbol.as_str().into() },
        RawKind::Url { expanded, .. } => SpanKind::Link { url: Url::parse(expanded).ok()? },
        RawKind::Media => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn spans(rt: &RichText) -> Vec<(&str, &SpanKind)> {
        rt.spans().iter().map(|s| (&rt.text()[s.range.clone()], &s.kind)).collect()
    }

    #[test]
    fn mentions_hashtags_and_links() {
        let raw = "hi @Ferris see #rust at https://t.co/abc ok";
        let ents = entities(&json!({
            "user_mentions": [{"screen_name": "ferris", "id_str": "1", "indices": [3, 10]}],
            "hashtags": [{"text": "rust", "indices": [15, 20]}],
            "urls": [{"url": "https://t.co/abc", "expanded_url": "https://rust-lang.org/learn", "display_url": "rust-lang.org/learn", "indices": [24, 40]}]
        }));
        let rt = rich_text(raw, ents, None);
        assert_eq!(rt.text(), "hi @Ferris see #rust at rust-lang.org/learn ok");
        let s = spans(&rt);
        assert_eq!(s[0].0, "@Ferris");
        assert!(matches!(s[0].1, SpanKind::Mention(u) if u.handle == "ferris"));
        assert_eq!(s[1].0, "#rust");
        assert_eq!(s[2].0, "rust-lang.org/learn");
        assert!(
            matches!(s[2].1, SpanKind::Link { url } if url.as_str() == "https://rust-lang.org/learn")
        );
    }

    #[test]
    fn code_point_offsets_with_emoji_and_cjk() {
        // 🦀 is one code point but two UTF-16 units and four bytes.
        let raw = "🦀🦀 #日本 @a_b";
        let ents = entities(&json!({
            "hashtags": [{"text": "日本", "indices": [3, 6]}],
            "user_mentions": [{"screen_name": "a_b", "indices": [7, 11]}]
        }));
        let rt = rich_text(raw, ents, None);
        let s = spans(&rt);
        assert_eq!(s[0].0, "#日本");
        assert_eq!(s[1].0, "@a_b");
    }

    #[test]
    fn display_range_drops_reply_prefix_and_media_link() {
        let raw = "@alice @bob thanks! https://t.co/pic";
        let ents = entities(&json!({
            "user_mentions": [
                {"screen_name": "alice", "indices": [0, 6]},
                {"screen_name": "bob", "indices": [7, 11]}
            ],
            "media": [{"url": "https://t.co/pic", "indices": [20, 36]}]
        }));
        let rt = rich_text(raw, ents, Some((12, 19)));
        assert_eq!(rt.text(), "thanks!");
        assert!(rt.spans().is_empty());
    }

    #[test]
    fn media_link_inside_range_is_removed_and_trailing_space_trimmed() {
        let raw = "look https://t.co/pic";
        let ents = entities(&json!({"media": [{"url": "https://t.co/pic", "indices": [5, 21]}]}));
        assert_eq!(rich_text(raw, ents, None).text(), "look");
    }

    #[test]
    fn html_entities_are_decoded_around_spans() {
        let raw = "a &amp; b #c &lt;3";
        let ents = entities(&json!({"hashtags": [{"text": "c", "indices": [10, 12]}]}));
        let rt = rich_text(raw, ents, None);
        assert_eq!(rt.text(), "a & b #c <3");
        assert_eq!(spans(&rt)[0].0, "#c");
    }

    #[test]
    fn bad_entities_are_dropped_not_fatal() {
        let raw = "short";
        let ents = entities(&json!({
            "hashtags": [{"text": "x", "indices": [2, 99]}, {"text": "y", "indices": [3, 1]}],
            "urls": [{"url": "u", "indices": [0, 2]}, {"url": "v", "expanded_url": "not a url", "indices": [1, 3]}]
        }));
        let rt = rich_text(raw, ents, None);
        assert_eq!(rt.text(), "uort", "first url shown as its display; overlapping one dropped");
        assert!(rt.spans().is_empty(), "unparsable URL gives no link span");
    }

    /// The text each typed entity covers.
    fn typed(raw: &str) -> Vec<String> {
        let chars: Vec<char> = raw.chars().collect();
        typed_entities(raw).iter().map(|e| chars[e.start..e.end].iter().collect()).collect()
    }

    #[test]
    fn typed_mentions_hashtags_and_cashtags() {
        assert_eq!(
            typed("@a hi (@b_2), #tag. $AAPL $BRK.A ＠wide ＃広い #日本語"),
            ["@a", "@b_2", "#tag", "$AAPL", "$BRK.A", "＠wide", "＃広い", "#日本語"]
        );
    }

    #[test]
    fn combining_marks_stay_in_a_hashtag() {
        assert_eq!(typed("#हिन्दी #ภาษาไทย"), ["#हिन्दी", "#ภาษาไทย"]);
    }

    #[test]
    fn addresses_and_lookalikes_are_not_typed_entities() {
        let none: [&str; 0] = [];
        for raw in [
            "me@example.com",
            "aysvczyothcf4@njkxf.mwy",
            "@toolonghandle_1234",
            "@a@b",
            "@a://x",
            "it&#39;s",
            "a#b",
            "#123",
            "#️⃣",
            "# alone",
            "@ alone",
            "$5 a$B $TOOLONG $a1",
        ] {
            assert_eq!(typed(raw), none, "{raw}");
        }
    }

    #[test]
    fn typed_entities_inside_twitters_are_skipped() {
        let raw = "see https://t.co/@x #ok";
        let ents = entities(&json!({"urls": [{"url": "https://t.co/@x",
            "expanded_url": "https://a.io/@x", "display_url": "a.io/@x", "indices": [4, 18]}]}));
        let rt = rich_text(raw, with_typed_entities(raw, ents), None);
        let s = spans(&rt);
        assert_eq!(s.iter().map(|s| s.0).collect::<Vec<_>>(), ["a.io/@x", "#ok"]);
    }

    fn link_ref(from: usize, to: usize, expanded: &str) -> Value {
        json!({"fromIndex": from, "toIndex": to, "ref": {
            "type": "TimelineUrl", "url": "https://t.co/x", "expandedUrl": expanded, "urlType": "ExternalUrl"
        }})
    }

    #[test]
    fn rich_message_links_keep_their_shown_text() {
        // Code points, after an astral emoji; the shown text is already shortened.
        let raw = "🦀 See example.com/a-lo… and t.co";
        let ents = vec![
            link_ref(6, 23, "https://example.com/a-long-path"),
            json!({"fromIndex": 28, "toIndex": 32, "ref": {"type": "TimelineUrl", "url": "https://t.co/y"}}),
        ];
        let rt = rich_text(raw, rich_message_entities(raw, &ents), None);
        assert_eq!(rt.text(), raw);
        let s = spans(&rt);
        assert_eq!(s[0].0, "example.com/a-lo…");
        assert!(
            matches!(s[0].1, SpanKind::Link { url } if url.as_str() == "https://example.com/a-long-path")
        );
        assert!(
            matches!(s[1].1, SpanKind::Link { url } if url.as_str() == "https://t.co/y"),
            "without expandedUrl, the url is used"
        );
    }

    #[test]
    fn rich_message_entities_that_arent_links_or_are_broken_are_dropped() {
        let raw = "hello world";
        let ents = vec![
            json!({"fromIndex": 0, "toIndex": 5, "ref": {"type": "TimelineRichTextUser"}}),
            link_ref(6, 99, "https://a.io"),
            link_ref(8, 6, "https://a.io"),
            json!({"fromIndex": "0", "toIndex": 5, "ref": {"type": "TimelineUrl", "url": "https://a.io"}}),
            json!({"fromIndex": 0, "toIndex": 5, "ref": {"type": "TimelineUrl"}}),
        ];
        assert!(rich_message_entities(raw, &ents).is_empty());
    }

    proptest! {
        /// Whatever offsets a rich message carries, translation never panics and spans stay valid.
        #[test]
        fn rich_message_never_panics_and_spans_are_valid(
            raw in "\\PC{0,30}",
            ents in proptest::collection::vec((0usize..40, 0usize..40), 0..6),
        ) {
            let ents: Vec<Value> = ents.into_iter().map(|(a, b)| link_ref(a, b, "https://d.io/")).collect();
            let rt = rich_text(&raw, rich_message_entities(&raw, &ents), None);
            for s in rt.spans() {
                prop_assert!(rt.text().get(s.range.clone()).is_some());
            }
        }
    }

    proptest! {
        /// Whatever a bio says, finding its entities never panics and gives
        /// valid, sorted, non-overlapping spans that start with their sigil.
        #[test]
        fn typed_entities_never_panic_and_spans_are_valid(
            raw in "([@#$＠＃&_:/. a-zA-Z0-9]|\\PC){0,40}",
            urls in proptest::collection::vec((0usize..45, 0usize..45), 0..3),
        ) {
            let ents = urls.into_iter().map(|(start, end)| RawEntity {
                start,
                end,
                kind: RawKind::Url { display: "d.io".into(), expanded: "https://d.io/".into() },
            }).collect();
            let rt = rich_text(&raw, with_typed_entities(&raw, ents), None);
            let mut last = 0;
            for s in rt.spans() {
                let covered = rt.text().get(s.range.clone());
                prop_assert!(covered.is_some());
                prop_assert!(s.range.start >= last);
                last = s.range.end;
                if !matches!(s.kind, SpanKind::Link { .. }) {
                    prop_assert!(covered.is_some_and(|c| c.starts_with(['@', '#', '$', '＠', '＃'])));
                }
            }
        }
    }

    proptest! {
        /// Whatever entities Twitter sends, translation never panics and spans stay valid.
        #[test]
        fn never_panics_and_spans_are_valid(
            raw in "\\PC{0,30}",
            ents in proptest::collection::vec((0usize..40, 0usize..40, 0u8..5), 0..6),
            range in proptest::option::of((0usize..40, 0usize..40)),
        ) {
            let ents = ents.into_iter().map(|(a, b, k)| RawEntity {
                start: a,
                end: b,
                kind: match k {
                    0 => RawKind::Mention { handle: "h".into(), id: None },
                    1 => RawKind::Hashtag("t".into()),
                    2 => RawKind::Cashtag("C".into()),
                    3 => RawKind::Url { display: "d.io".into(), expanded: "https://d.io/".into() },
                    _ => RawKind::Media,
                },
            }).collect();
            let rt = rich_text(&raw, ents, range);
            for s in rt.spans() {
                prop_assert!(rt.text().get(s.range.clone()).is_some());
            }
        }
    }
}
