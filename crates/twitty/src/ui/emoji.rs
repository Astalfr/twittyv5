//! Twemoji: emoji drawn with Twitter's own artwork (Mozilla's COLR build of
//! it) instead of Segoe UI Emoji. The ranges come from `state::emoji`.

use std::{borrow::Cow, ops::Range};

use gpui_kit::{App, HighlightStyle, SharedString, StyledText};

use crate::state::emoji;

/// The family name inside the bundled font.
pub const FAMILY: &str = "Twemoji Mozilla";
/// Twemoji Mozilla v0.7.0 (emoji up to Unicode 14); see `resources/fonts/README.md`.
const FONT: &[u8] = include_bytes!("../../resources/fonts/Twemoji.Mozilla.ttf");

/// Registers the bundled font. If that fails, emoji fall back to the system's.
pub fn load_font(cx: &App) {
    if let Err(e) = cx.text_system().add_fonts(vec![Cow::Borrowed(FONT)]) {
        tracing::warn!("could not load the Twemoji font, using the system emoji: {e:#}");
    }
}

/// Plain text with its emoji drawn in Twemoji.
pub fn text(text: impl Into<SharedString>) -> StyledText {
    styled(text.into(), &[], |_| HighlightStyle::default())
}

/// `text` with `spans` (sorted, non-overlapping) highlighted by `style`,
/// which gets the span's index, and its emoji drawn in Twemoji.
pub fn styled(
    text: SharedString,
    spans: &[Range<usize>],
    style: impl Fn(usize) -> HighlightStyle,
) -> StyledText {
    let ranges = emoji::emoji_ranges(&text);
    // GPUI only switches the font of whole runs, and cuts runs where
    // highlights start and end: every emoji range needs its own highlight.
    let highlights: Vec<_> = emoji::segments(spans, &ranges)
        .into_iter()
        .map(|s| (s.range, s.span.map(&style).unwrap_or_default()))
        .collect();
    let family = SharedString::new_static(FAMILY);
    StyledText::new(text)
        .with_highlights(highlights)
        .with_font_family_overrides(ranges.into_iter().map(|r| (r, family.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    use ttf_parser::{Face, name_id};

    #[test]
    fn family_matches_the_bundled_font() {
        let face = Face::parse(FONT, 0).unwrap();
        let families: Vec<String> = face
            .names()
            .into_iter()
            .filter(|n| n.name_id == name_id::FAMILY)
            .filter_map(|n| n.to_string())
            .collect();
        assert!(families.iter().any(|f| f == FAMILY), "{families:?}");
    }

    /// GPUI sizes a glyph from its own outline before painting the colour
    /// layers, so an emoji whose glyph has no outline is never drawn. The
    /// upstream font has none (see `resources/fonts/fill_base_glyphs.py`).
    #[test]
    fn emoji_glyphs_have_outlines() {
        let face = Face::parse(FONT, 0).unwrap();
        let mut checked = 0;
        for c in ('\u{1F300}'..='\u{1FAFF}').chain('\u{2600}'..='\u{27BF}') {
            let Some(glyph) = face.glyph_index(c) else { continue };
            assert!(face.glyph_bounding_box(glyph).is_some(), "U+{:X} has no outline", c as u32);
            checked += 1;
        }
        assert!(checked > 1000, "only {checked} emoji in the font");
    }
}
