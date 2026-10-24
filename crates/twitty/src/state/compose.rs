//! Composing a tweet: what it answers or quotes, Twitter's weighted length limit and
//! the draft sent to the backend.
//!
//! Twitter counts text with twitter-text's weighting: characters in the ranges below
//! count 1, everything else (CJK, emoji…) counts 2, and every link counts 23
//! whatever its length. The limit is 280. Emoji sequences joined with ZWJ are
//! counted per code point here, so they can read slightly high.

use std::sync::Arc;

use twitty_model::{Draft, Post};

/// What a tweet being composed is about.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ComposeTarget {
    /// A new tweet on its own.
    #[default]
    New,
    /// A reply to this tweet.
    Reply(Arc<Post>),
    /// A quote of this tweet: the new tweet embeds it.
    Quote(Arc<Post>),
}

impl ComposeTarget {
    /// The tweet replied to, if any.
    pub fn reply_to(&self) -> Option<&Arc<Post>> {
        match self {
            Self::Reply(post) => Some(post),
            _ => None,
        }
    }

    /// The tweet quoted, if any.
    pub fn quoted(&self) -> Option<&Arc<Post>> {
        match self {
            Self::Quote(post) => Some(post),
            _ => None,
        }
    }
}

/// Maximum weighted length of a tweet.
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

/// The length Twitter counts for `text`.
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

/// Whether `text` can be tweeted.
pub fn can_post(text: &str) -> bool {
    !text.trim().is_empty() && remaining(text) >= 0
}

/// The draft for `text`, replying to or quoting `target`'s tweet.
pub fn draft(text: &str, target: &ComposeTarget) -> Draft {
    Draft {
        text: text.trim().to_owned(),
        reply_to: target.reply_to().map(|p| p.id.clone()),
        quote: target.quoted().map(|p| p.id.clone()),
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
        let d = draft("  hi  ", &ComposeTarget::New);
        assert_eq!((d.text.as_str(), d.reply_to, d.quote), ("hi", None, None));
    }

    fn some_post() -> Arc<Post> {
        let backend = twitty_model::mock::MockBackend::new();
        let draft = Draft { text: "parent".into(), ..Default::default() };
        let post = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(twitty_model::Backend::publish(&backend, draft))
            .unwrap();
        Arc::new(post)
    }

    #[test]
    fn drafts_carry_the_reply_or_the_quote() {
        let post = some_post();
        let reply = draft("a reply", &ComposeTarget::Reply(Arc::clone(&post)));
        assert_eq!((reply.reply_to.as_ref(), reply.quote), (Some(&post.id), None));
        let quote = draft("a quote", &ComposeTarget::Quote(Arc::clone(&post)));
        assert_eq!((quote.reply_to, quote.quote.as_ref()), (None, Some(&post.id)));
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
