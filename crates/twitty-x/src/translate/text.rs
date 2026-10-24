//! Post and bio text: X's raw text + entity list → `RichText`.
//!
//! X gives entity offsets in **Unicode code points** of the raw (HTML-escaped)
//! text, verified against live data on 2026-10-24. `display_text_range`
//! (also code points) cuts the leading reply mentions and the trailing media
//! link. Output spans are byte ranges into the decoded text.

use serde_json::Value;
use twitty_model::{RichText, Span, SpanKind, UserId, UserRef};
use url::Url;

use crate::wire::{array_at, str_at};

/// An entity as X describes it, before translation.
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
    /// The trailing `t.co` link X appends for attached media: removed.
    Media,
}

fn indices(e: &Value) -> Option<(usize, usize)> {
    let i = array_at(e, &["indices"]);
    let a = usize::try_from(i.first()?.as_u64()?).ok()?;
    let b = usize::try_from(i.get(1)?.as_u64()?).ok()?;
    (a < b).then_some((a, b))
}

/// Reads an X entity object (`entities`, `entity_set`, bio `description`).
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

    proptest! {
        /// Whatever entities X sends, translation never panics and spans stay valid.
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
