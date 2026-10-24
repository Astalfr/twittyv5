//! Composing a post: X's weighted length limit and the draft sent to the backend.
//!
//! X counts text with twitter-text's weighting: characters in the ranges below
//! count 1, everything else (CJK, emoji…) counts 2, and every link counts 23
//! whatever its length. The limit is 280. Emoji sequences joined with ZWJ are
//! counted per code point here, so they can read slightly high.

use twitty_model::{Draft, Post, PostId};

/// Maximum weighted length of a post.
pub const MAX_LENGTH: usize = 280;
/// Weighted length of any link.
const LINK_LENGTH: usize = 23;

/// Code point ranges that count 1 (twitter-text v3 config).
const LIGHT: [(u32, u32); 4] = [(0, 4351), (8192, 8205), (8208, 8223), (8242, 8247)];

fn weight(c: char) -> usize {
    let c = u32::from(c);
    if LIGHT.iter().any(|&(lo, hi)| (lo..=hi).contains(&c)) { 1 } else { 2 }
}

fn is_link(word: &str) -> bool {
    let rest = word.strip_prefix("https://").or_else(|| word.strip_prefix("http://"));
    rest.is_some_and(|r| r.contains('.'))
}

/// The length X counts for `text`.
pub fn weighted_length(text: &str) -> usize {
    let mut total = 0;
    let mut word = String::new();
    let flush = |word: &mut String, total: &mut usize| {
        *total += if is_link(word) { LINK_LENGTH } else { word.chars().map(weight).sum() };
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut total);
            total += weight(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut total);
    total
}

/// Characters left before the limit (negative when over).
pub fn remaining(text: &str) -> isize {
    let used = isize::try_from(weighted_length(text)).unwrap_or(isize::MAX);
    isize::try_from(MAX_LENGTH).unwrap_or(isize::MAX) - used
}

/// Whether `text` can be posted.
pub fn can_post(text: &str) -> bool {
    !text.trim().is_empty() && remaining(text) >= 0
}

/// The draft for `text`, replying to `reply_to` if set.
pub fn draft(text: &str, reply_to: Option<&Post>) -> Draft {
    Draft {
        text: text.trim().to_owned(),
        reply_to: reply_to.map(|p| p.id.clone()),
        quote: None::<PostId>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn latin_counts_one_cjk_and_emoji_two() {
        assert_eq!(weighted_length("hello"), 5);
        assert_eq!(weighted_length("日本語"), 6);
        assert_eq!(weighted_length("🚀"), 2);
        assert_eq!(weighted_length("a b"), 3);
    }

    #[test]
    fn links_count_23() {
        assert_eq!(weighted_length("https://example.com/a/very/long/path?with=query"), 23);
        assert_eq!(weighted_length("see http://x.co"), 4 + 23);
        assert_eq!(weighted_length("https://"), 8, "not a link without a host");
    }

    #[test]
    fn limit_is_280() {
        assert!(can_post(&"a".repeat(280)));
        assert!(!can_post(&"a".repeat(281)));
        assert_eq!(remaining(&"日".repeat(141)), -2);
        assert!(!can_post("   \n "), "blank");
    }

    #[test]
    fn drafts_trim_and_reply() {
        let d = draft("  hi  ", None);
        assert_eq!((d.text.as_str(), d.reply_to), ("hi", None));
    }

    proptest! {
        #[test]
        fn length_is_between_chars_and_twice_chars(s in "\\PC{0,200}") {
            let n = weighted_length(&s);
            let chars = s.chars().count();
            // Links can only shorten long words to 23.
            prop_assert!(n <= 2 * chars.max(1) + LINK_LENGTH);
            if !s.contains("http") {
                prop_assert!(n >= chars && n <= 2 * chars);
            }
        }
    }
}
