use std::ops::Range;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use url::Url;

use crate::UserId;

/// Text with annotated spans (mentions, hashtags, links).
///
/// Invariants, enforced by [`RichText::new`]:
/// - every span range lies within `text` and on `char` boundaries,
/// - spans are non-empty, sorted by start and do not overlap.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "RawRichText", into = "RawRichText")]
pub struct RichText {
    text: String,
    spans: Vec<Span>,
}

/// An annotated byte range of a [`RichText`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Byte range into the owning text.
    pub range: Range<usize>,
    /// What the range refers to.
    pub kind: SpanKind,
}

/// The meaning of a [`Span`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpanKind {
    /// `@handle`.
    Mention(UserRef),
    /// `#tag` (without the `#`).
    Hashtag { tag: SmolStr },
    /// `$TICKER` (without the `$`).
    Cashtag { symbol: SmolStr },
    /// A link; the covered text is the display form.
    Link { url: Url },
}

/// A lightweight reference to a user, as found in mentions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    /// Present when the backend knows the account id.
    pub id: Option<UserId>,
    /// Handle without the leading `@`.
    pub handle: SmolStr,
}

/// Why a [`RichText`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RichTextError {
    /// A span is empty or reaches past the end of the text.
    #[error("span {range:?} is empty or out of bounds for text of {len} bytes")]
    OutOfBounds { range: Range<usize>, len: usize },
    /// A span boundary splits a UTF-8 character.
    #[error("span {range:?} does not fall on character boundaries")]
    NotCharBoundary { range: Range<usize> },
    /// Two spans overlap or are out of order.
    #[error("span {range:?} overlaps or precedes the previous span")]
    Overlap { range: Range<usize> },
}

/// A run of text, either plain or covered by one span. See [`RichText::segments`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment<'a> {
    /// The text of this run.
    pub text: &'a str,
    /// The span covering the run, if any.
    pub kind: Option<&'a SpanKind>,
}

impl RichText {
    /// Builds rich text, validating the span invariants.
    pub fn new(text: impl Into<String>, mut spans: Vec<Span>) -> Result<Self, RichTextError> {
        let text = text.into();
        spans.sort_by_key(|s| s.range.start);
        let mut prev_end = 0;
        for span in &spans {
            let r = &span.range;
            if r.start >= r.end || r.end > text.len() {
                return Err(RichTextError::OutOfBounds { range: r.clone(), len: text.len() });
            }
            if !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
                return Err(RichTextError::NotCharBoundary { range: r.clone() });
            }
            if r.start < prev_end {
                return Err(RichTextError::Overlap { range: r.clone() });
            }
            prev_end = r.end;
        }
        Ok(Self { text, spans })
    }

    /// Text without any annotation.
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), spans: Vec::new() }
    }

    /// The full text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The spans, sorted and non-overlapping.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Splits the text into consecutive runs covering all of it, in order.
    pub fn segments(&self) -> Vec<Segment<'_>> {
        let mut out = Vec::with_capacity(self.spans.len() * 2 + 1);
        let mut cursor = 0;
        for span in &self.spans {
            if span.range.start > cursor {
                out.push(Segment { text: &self.text[cursor..span.range.start], kind: None });
            }
            out.push(Segment { text: &self.text[span.range.clone()], kind: Some(&span.kind) });
            cursor = span.range.end;
        }
        if cursor < self.text.len() {
            out.push(Segment { text: &self.text[cursor..], kind: None });
        }
        out
    }
}

/// Serialized form; deserialization goes through [`RichText::new`] so the
/// invariants hold for data read from disk too.
#[derive(Serialize, Deserialize)]
struct RawRichText {
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    spans: Vec<Span>,
}

impl TryFrom<RawRichText> for RichText {
    type Error = RichTextError;
    fn try_from(raw: RawRichText) -> Result<Self, Self::Error> {
        RichText::new(raw.text, raw.spans)
    }
}

impl From<RichText> for RawRichText {
    fn from(rt: RichText) -> Self {
        RawRichText { text: rt.text, spans: rt.spans }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tag(range: Range<usize>) -> Span {
        Span { range, kind: SpanKind::Hashtag { tag: "t".into() } }
    }

    #[test]
    fn new_sorts_spans() {
        let rt = RichText::new("#a #b", vec![tag(3..5), tag(0..2)]).unwrap();
        assert_eq!(rt.spans()[0].range, 0..2);
    }

    #[test]
    fn new_rejects_out_of_bounds() {
        let err = RichText::new("abc", vec![tag(1..4)]).unwrap_err();
        assert!(matches!(err, RichTextError::OutOfBounds { .. }));
    }

    #[test]
    fn new_rejects_empty_span() {
        assert!(RichText::new("abc", vec![tag(1..1)]).is_err());
    }

    #[test]
    fn new_rejects_split_char() {
        // "é" is two bytes.
        let err = RichText::new("é", vec![tag(0..1)]).unwrap_err();
        assert!(matches!(err, RichTextError::NotCharBoundary { .. }));
    }

    #[test]
    fn new_rejects_overlap() {
        let err = RichText::new("abcdef", vec![tag(0..3), tag(2..4)]).unwrap_err();
        assert!(matches!(err, RichTextError::Overlap { .. }));
    }

    #[test]
    fn segments_cover_text_in_order() {
        let rt = RichText::new("hi #rust ok", vec![tag(3..8)]).unwrap();
        let segs = rt.segments();
        let texts: Vec<_> = segs.iter().map(|s| s.text).collect();
        assert_eq!(texts, ["hi ", "#rust", " ok"]);
        assert!(segs[1].kind.is_some());
    }

    #[test]
    fn deserialize_validates_invariants() {
        let bad = r#"{"text":"ab","spans":[{"range":{"start":0,"end":9},"kind":{"type":"hashtag","tag":"x"}}]}"#;
        assert!(serde_json::from_str::<RichText>(bad).is_err());
    }

    #[test]
    fn serde_round_trip() {
        let rt = RichText::new(
            "@bob see https://x.y",
            vec![
                Span {
                    range: 0..4,
                    kind: SpanKind::Mention(UserRef { id: None, handle: "bob".into() }),
                },
                Span {
                    range: 9..20,
                    kind: SpanKind::Link { url: Url::parse("https://x.y/").unwrap() },
                },
            ],
        )
        .unwrap();
        let json = serde_json::to_string(&rt).unwrap();
        assert_eq!(serde_json::from_str::<RichText>(&json).unwrap(), rt);
    }

    proptest! {
        #[test]
        fn segments_concatenate_to_text(text in "\\PC{0,40}", cuts in proptest::collection::vec(0usize..200, 0..8)) {
            // Build valid, non-overlapping spans on char boundaries from random cut points.
            let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
            let mut points: Vec<usize> = cuts.iter().map(|c| bounds[c % bounds.len()]).collect();
            points.sort_unstable();
            points.dedup();
            let spans: Vec<Span> = points.as_chunks::<2>().0.iter().map(|[a, b]| tag(*a..*b)).collect();
            let rt = RichText::new(text.clone(), spans).unwrap();
            let joined: String = rt.segments().iter().map(|s| s.text).collect();
            prop_assert_eq!(joined, text);
        }
    }
}
