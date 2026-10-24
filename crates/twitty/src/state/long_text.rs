//! Long tweets (Twitter Blue's "note tweets"): outside the thread panel their
//! text is cut after about a classic tweet's length, with a "Show more" link
//! under it. The viewer can unfold each tweet, and every column follows.

use std::{borrow::Cow, collections::HashSet};

use twitty_model::{PostId, RichText};

/// Where a long text is cut, in characters: a classic tweet's length.
pub const CUT: usize = 280;
/// Texts up to this many characters are shown whole. Above [`CUT`] so a
/// classic tweet whose links display longer than Twitter counts them, or a
/// translation a little longer than its original, is never folded.
pub const FOLD_ABOVE: usize = 320;
/// The link that unfolds a cut text.
pub const SHOW_MORE: &str = "Show more";
/// The link that folds it again.
pub const SHOW_LESS: &str = "Show less";
/// A cut is moved back to the last space only if that loses fewer
/// characters than this, so a text without spaces is still cut near [`CUT`].
const WORD_SLACK: usize = 40;

/// Tweets the viewer unfolded.
#[derive(Debug, Clone, Default)]
pub struct Unfolded(HashSet<PostId>);

impl Unfolded {
    pub fn is_unfolded(&self, id: &PostId) -> bool {
        self.0.contains(id)
    }

    /// Unfolds `id`, or folds it again.
    pub fn toggle(&mut self, id: &PostId) {
        if !self.0.remove(id) {
            self.0.insert(id.clone());
        }
    }
}

/// The text to render for a tweet, and the link under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Folded<'a> {
    pub body: Cow<'a, RichText>,
    /// [`SHOW_MORE`], [`SHOW_LESS`], or `None` for a text short enough to
    /// always show whole.
    pub action: Option<&'static str>,
}

/// What to show of `body`, given whether the viewer unfolded it.
pub fn fold(body: &RichText, unfolded: bool) -> Folded<'_> {
    match cut(body) {
        None => Folded { body: Cow::Borrowed(body), action: None },
        Some(_) if unfolded => Folded { body: Cow::Borrowed(body), action: Some(SHOW_LESS) },
        Some(short) => Folded { body: Cow::Owned(short), action: Some(SHOW_MORE) },
    }
}

/// `text` cut after about [`CUT`] characters and ended with "…", or `None`
/// when it has at most [`FOLD_ABOVE`]. The cut falls between words when it
/// can and never inside a link, mention or hashtag; spans after it are dropped.
pub fn cut(text: &RichText) -> Option<RichText> {
    let s = text.text();
    // Short texts have no character past `FOLD_ABOVE`.
    s.chars().nth(FOLD_ABOVE)?;
    let mut end = s.char_indices().nth(CUT).map_or(s.len(), |(i, _)| i);
    if let Some(span) = text.spans().iter().find(|sp| sp.range.start < end && end < sp.range.end) {
        end = span.range.start;
    }
    if let Some(space) = s[..end].rfind(char::is_whitespace)
        && s[space..end].chars().count() < WORD_SLACK
    {
        end = space;
    }
    let kept = s[..end].trim_end();
    let spans = text.spans().iter().filter(|sp| sp.range.end <= kept.len()).cloned().collect();
    let short = format!("{kept}…");
    // The kept spans are unchanged and end before the cut, so they stay valid.
    Some(RichText::new(short.clone(), spans).unwrap_or_else(|_| RichText::plain(short)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use twitty_model::{Span, SpanKind};

    fn link(range: std::ops::Range<usize>) -> Span {
        let url = "https://example.com".parse().unwrap();
        Span { range, kind: SpanKind::Link { url } }
    }

    #[test]
    fn a_classic_tweet_is_never_folded() {
        let text = RichText::plain("a".repeat(FOLD_ABOVE));
        assert_eq!(cut(&text), None);
        assert_eq!(fold(&text, false), Folded { body: Cow::Borrowed(&text), action: None });
    }

    #[test]
    fn a_long_text_is_cut_between_words() {
        let text = RichText::plain("word ".repeat(100));
        let short = cut(&text).unwrap();
        assert!(short.text().ends_with("word…"), "{:?}", short.text());
        assert!(short.text().chars().count() <= CUT + 1);
        assert!(short.text().chars().count() > CUT - WORD_SLACK);
    }

    #[test]
    fn a_text_without_spaces_is_cut_at_the_limit() {
        let text = RichText::plain("あ".repeat(400));
        let short = cut(&text).unwrap();
        assert_eq!(short.text(), format!("{}…", "あ".repeat(CUT)));
    }

    #[test]
    fn the_cut_never_splits_a_link() {
        let s = format!("{}{}{}", "a".repeat(270), "x".repeat(20), "b".repeat(100));
        let text = RichText::new(s, vec![link(270..290)]).unwrap();
        let short = cut(&text).unwrap();
        assert_eq!(short.text(), format!("{}…", "a".repeat(270)));
        assert!(short.spans().is_empty(), "the link is left out, not cut");
    }

    #[test]
    fn spans_before_the_cut_are_kept() {
        let s = format!("{}{}", "x".repeat(10), " y".repeat(300));
        let text = RichText::new(s, vec![link(0..10)]).unwrap();
        assert_eq!(cut(&text).unwrap().spans(), &[link(0..10)]);
    }

    #[test]
    fn unfolding_shows_the_whole_text_and_folds_back() {
        let text = RichText::plain("word ".repeat(100));
        let folded = fold(&text, false);
        assert_eq!(folded.action, Some(SHOW_MORE));
        assert_ne!(*folded.body, text);
        let unfolded = fold(&text, true);
        assert_eq!(unfolded, Folded { body: Cow::Borrowed(&text), action: Some(SHOW_LESS) });
    }

    #[test]
    fn toggling_unfolds_then_folds() {
        let id = PostId::new("1");
        let mut unfolded = Unfolded::default();
        unfolded.toggle(&id);
        assert!(unfolded.is_unfolded(&id));
        unfolded.toggle(&id);
        assert!(!unfolded.is_unfolded(&id));
    }

    proptest! {
        #[test]
        fn cutting_never_panics_and_keeps_valid_spans(
            words in proptest::collection::vec("[a-zé😀 \n]{0,12}", 0..80),
            every in 1usize..6,
        ) {
            let s: String = words.concat();
            // A link over every `every`-th char-aligned word.
            let mut spans = Vec::new();
            let mut at = 0;
            for (k, w) in words.iter().enumerate() {
                if k % every == 0 && !w.is_empty() {
                    spans.push(link(at..at + w.len()));
                }
                at += w.len();
            }
            let text = RichText::new(s.clone(), spans).unwrap();
            if let Some(short) = cut(&text) {
                prop_assert!(short.text().chars().count() <= CUT + 1);
                prop_assert!(short.text().ends_with('…'));
                let kept = short.text().trim_end_matches('…');
                prop_assert!(s.starts_with(kept));
                for sp in short.spans() {
                    prop_assert!(sp.range.end <= kept.len());
                    prop_assert!(text.spans().contains(sp));
                }
            } else {
                prop_assert!(s.chars().count() <= FOLD_ABOVE);
            }
        }
    }
}
