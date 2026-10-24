//! Where a text shows emoji, so the views can draw them with the bundled
//! Twemoji font instead of Segoe UI Emoji.
//!
//! GPUI can only switch fonts for whole text runs, and only cuts runs where
//! highlights start and end, so the views also need [`segments`]: the link
//! spans and emoji ranges merged into pieces that each get one style.

use std::ops::Range;

use unicode_properties::{EmojiStatus, UnicodeEmoji};
use unicode_segmentation::UnicodeSegmentation;

/// VARIATION SELECTOR-16: asks for the emoji look of the character before it.
const EMOJI_PRESENTATION: char = '\u{FE0F}';
/// VARIATION SELECTOR-15: asks for the text look.
const TEXT_PRESENTATION: char = '\u{FE0E}';
/// COMBINING ENCLOSING KEYCAP, as in 1️⃣.
const KEYCAP: char = '\u{20E3}';

/// The byte ranges of `text` that are drawn as emoji, sorted. Neighbouring
/// emoji are merged into one range.
pub fn emoji_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (start, cluster) in text.grapheme_indices(true) {
        if !is_emoji(cluster) {
            continue;
        }
        let end = start + cluster.len();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

/// Whether a grapheme cluster is drawn with Twemoji. Digits, `#`, `©` and
/// many symbols have Unicode's `Emoji` property but show as text by default,
/// so they only count with a VS16.
///
/// Keycaps stay with the system font: GPUI doesn't apply Twemoji's keycap
/// ligature, and the font's lone `1`, VS16 and keycap glyphs are empty.
fn is_emoji(cluster: &str) -> bool {
    let Some(first) = cluster.chars().next() else { return false };
    if !first.is_emoji_char() || cluster.contains([TEXT_PRESENTATION, KEYCAP]) {
        return false;
    }
    cluster.chars().any(|c| {
        c == EMOJI_PRESENTATION
            || matches!(
                c.emoji_status(),
                EmojiStatus::EmojiPresentation
                    | EmojiStatus::EmojiPresentationAndModifierBase
                    | EmojiStatus::EmojiPresentationAndEmojiComponent
                    | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
            )
    })
}

/// A piece of text drawn as one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    /// The index of the span (link, mention…) it belongs to, if any.
    pub span: Option<usize>,
    /// Drawn with the emoji font.
    pub emoji: bool,
}

/// Cuts `spans` and `emoji` (each sorted and non-overlapping) wherever either
/// starts or ends. Text outside both is left out. The result is sorted and
/// non-overlapping, and covers every span and every emoji range exactly.
pub fn segments(spans: &[Range<usize>], emoji: &[Range<usize>]) -> Vec<Segment> {
    let mut cuts: Vec<usize> = spans.iter().chain(emoji).flat_map(|r| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|pair| {
            let [start, end] = [pair[0], pair[1]];
            let span = spans.iter().position(|r| r.contains(&start));
            let emoji = emoji.iter().any(|r| r.contains(&start));
            (span.is_some() || emoji).then_some(Segment { range: start..end, span, emoji })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    fn emoji_texts(text: &str) -> Vec<&str> {
        emoji_ranges(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn finds_emoji_in_a_display_name() {
        assert_eq!(emoji_texts("Al 🦀"), ["🦀"]);
        assert_eq!(emoji_texts("Emoji 🦀 Fan"), ["🦀"]);
    }

    #[test]
    fn neighbouring_emoji_make_one_range() {
        assert_eq!(emoji_texts("go 🦀🚀 now 🎉"), ["🦀🚀", "🎉"]);
    }

    #[test]
    fn sequences_stay_whole() {
        for seq in [
            "👩\u{200D}👩\u{200D}👧",                                   // ZWJ family
            "👍🏽",                                                       // skin tone
            "🇫🇷",                                                       // flag
            "❤\u{FE0F}", // text-default heart asked as emoji
            "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}", // England
        ] {
            assert_eq!(emoji_texts(&format!("a {seq} b")), [seq], "{seq:?}");
        }
    }

    #[test]
    fn text_default_symbols_and_keycaps_stay_text() {
        for text in [
            "#rust 2026",
            "© ® ™",
            "❤ without VS16",
            "☺\u{FE0E}",
            "*bold*",
            "1\u{FE0F}\u{20E3}",
            "#\u{20E3}",
        ] {
            assert_eq!(emoji_texts(text), Vec::<&str>::new(), "{text:?}");
        }
    }

    #[test]
    fn cjk_and_rtl_are_not_emoji() {
        assert_eq!(emoji_texts("日本語のテキスト שלום"), Vec::<&str>::new());
    }

    #[test]
    fn splits_a_link_around_its_emoji() {
        // "see #rust🦀 now": the hashtag span holds the emoji.
        let text = "see #rust🦀 now";
        let tag = 4..13;
        assert_eq!(&text[tag.clone()], "#rust🦀");
        assert_eq!(
            segments(&[tag], &emoji_ranges(text)),
            [
                Segment { range: 4..9, span: Some(0), emoji: false },
                Segment { range: 9..13, span: Some(0), emoji: true },
            ]
        );
    }

    #[test]
    fn emoji_outside_spans_get_their_own_segment() {
        let text = "🦀 @al hi";
        let mention = 5..8;
        assert_eq!(&text[mention.clone()], "@al");
        assert_eq!(
            segments(&[mention], &emoji_ranges(text)),
            [
                Segment { range: 0..4, span: None, emoji: true },
                Segment { range: 5..8, span: Some(0), emoji: false },
            ]
        );
    }

    /// Sorted, non-overlapping, non-empty ranges on `text`'s char boundaries.
    fn spans_in(text: &str, cuts: &[usize]) -> Vec<Range<usize>> {
        let mut bounds: Vec<usize> = cuts
            .iter()
            .map(|c| c % (text.len() + 1))
            .filter(|&c| text.is_char_boundary(c))
            .collect();
        bounds.sort_unstable();
        bounds.dedup();
        bounds.as_chunks::<2>().0.iter().map(|[start, end]| *start..*end).collect()
    }

    fn covered(ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<usize> {
        ranges.into_iter().flatten().collect()
    }

    proptest! {
        #[test]
        fn emoji_ranges_are_sorted_and_on_char_boundaries(
            text in "(\\PC|🦀|👍🏽|🇫🇷|1\u{FE0F}\u{20E3}|\u{200D}|\u{FE0F}|\u{FE0E}){0,40}",
        ) {
            let ranges = emoji_ranges(&text);
            for r in &ranges {
                prop_assert!(r.start < r.end && r.end <= text.len());
                prop_assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
            }
            for pair in ranges.windows(2) {
                // Touching ranges would have been merged.
                prop_assert!(pair[0].end < pair[1].start);
            }
        }

        #[test]
        fn segments_cover_spans_and_emoji_exactly(
            text in "(\\PC|🦀|👍🏽|🇫🇷){0,40}",
            cuts in prop::collection::vec(any::<usize>(), 0..12),
        ) {
            let spans = spans_in(&text, &cuts);
            let emoji = emoji_ranges(&text);
            let segs = segments(&spans, &emoji);
            for s in &segs {
                prop_assert!(s.range.start < s.range.end && s.range.end <= text.len());
                prop_assert!(text.is_char_boundary(s.range.start));
                prop_assert!(text.is_char_boundary(s.range.end));
            }
            for pair in segs.windows(2) {
                prop_assert!(pair[0].range.end <= pair[1].range.start);
            }
            for (ix, span) in spans.iter().enumerate() {
                let inside = segs.iter().filter(|s| s.span == Some(ix)).map(|s| s.range.clone());
                prop_assert_eq!(covered(inside), covered([span.clone()]));
            }
            let drawn_as_emoji = segs.iter().filter(|s| s.emoji).map(|s| s.range.clone());
            prop_assert_eq!(covered(drawn_as_emoji), covered(emoji));
        }
    }
}
