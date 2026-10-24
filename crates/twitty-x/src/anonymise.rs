//! Scrubs captured Twitter responses so they can be committed as test fixtures.
//!
//! The output keeps the JSON structure, every key, all numbers and the length
//! (in Unicode code points) of every string, so entity offsets stay valid.
//! Text, names, handles, ids, URLs and opaque tokens are replaced with
//! pseudo-random characters. The replacement of a word or id is the same
//! everywhere in one run (a handle in `screen_name` matches the `@handle` in a
//! tweet's text, an id in `id_str` matches the one in an `entryId`), but the
//! key is random per run, so the mapping can't be reversed with a dictionary.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::LazyLock,
};

use regex::{Captures, Regex};
use serde_json::Value;

/// Keys whose string values are human text: every word is scrambled.
const TEXT_KEYS: &[&str] = &[
    "full_text",
    "text",
    "description",
    "name",
    "screen_name",
    "location",
    "title",
    "content",
    "bodyText",
    "string_value",
    "display_name",
    "vanity",
    "alt_text",
    "ext_alt_text",
    // Grok's translation of a tweet (`grok_translated_post_with_availability`).
    "translation",
    "preview_translation",
];

/// Keys whose values are opaque tokens (cursors, base64 ids…): fully scrambled.
const OPAQUE_KEYS: &[&str] = &[
    "value",
    "id",
    "rest_id",
    "metadata",
    "impressionId",
    "controllerData",
    "sourceData",
    "media_key",
];

/// Keys kept verbatim (dates the translator must still parse).
const KEEP_KEYS: &[&str] = &["created_at", "timestamp_ms", "__typename", "type", "lang"];

/// Key fragments that mark personal values (`cash_app_handle`, `username`…).
const SENSITIVE_KEY_PARTS: &[&str] =
    &["handle", "name", "email", "address", "bitcoin", "ethereum", "phone"];

/// Hosts kept verbatim: Twitter's own, which identify nothing.
const KEEP_HOSTS: &[&str] = &[
    "x.com",
    "twitter.com",
    "t.co",
    "pbs.twimg.com",
    "video.twimg.com",
    "abs.twimg.com",
    "pic.x.com",
    "pic.twitter.com",
];

/// Path segments of URLs that describe routes, not people or content.
const ROUTE_WORDS: &[&str] = &[
    "status",
    "statuses",
    "media",
    "profile",
    "images",
    "banners",
    "ext",
    "tw",
    "video",
    "videos",
    "thumb",
    "amplify",
    "img",
    "vid",
    "pu",
    "avc1",
    "i",
    "web",
    "photo",
    "hashtag",
    "search",
    "tweet",
    "card",
    "broadcasts",
    "jpg",
    "jpeg",
    "png",
    "webp",
    "gif",
    "mp4",
    "m3u8",
    "small",
    "medium",
    "large",
    "orig",
    "normal",
    "bigger",
    "mini",
    "x96",
    "name",
    "format",
    "tag",
    "www",
    "http",
    "https",
];

static URL_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| re(r"https?://[^\s]+"));
static HTML_ENTITY: LazyLock<Regex> = LazyLock::new(|| re(r"&(?:amp|lt|gt|quot|apos|#\d+);"));
static DIGIT_RUN: LazyLock<Regex> = LazyLock::new(|| re(r"\d{6,}"));
static WORD: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{Alphabetic}\p{Nd}]+"));

#[expect(clippy::expect_used, reason = "patterns are constants covered by tests")]
fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

/// A scrubbing pass with its own random key.
#[derive(Debug, Clone, Copy)]
pub struct Anonymiser {
    key: u64,
}

impl Anonymiser {
    /// A pass with a fresh random key.
    pub fn random() -> Self {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let mut key = [0u8; 8];
        key.copy_from_slice(&bytes[..8]);
        Self { key: u64::from_le_bytes(key) }
    }

    /// A pass with a fixed key (tests only: fixed keys are reversible).
    pub fn with_key(key: u64) -> Self {
        Self { key }
    }

    /// Scrubs `value` in place.
    pub fn anonymise(&self, value: &mut Value) {
        self.walk(value, &mut Vec::new());
    }

    /// `path` holds the object keys from the root to `value` (arrays add nothing).
    fn walk(&self, value: &mut Value, path: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    path.push(k.clone());
                    self.walk(v, path);
                    path.pop();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.walk(v, path)),
            Value::String(s) => {
                // Card type names (`summary_large_image`, `unified_card`…) drive translation.
                let card_name =
                    path.len() >= 3 && path[path.len() - 3..] == ["card", "legacy", "name"];
                if !card_name {
                    *s = self.string(path.last().map(String::as_str).unwrap_or_default(), s);
                }
            }
            _ => {}
        }
    }

    fn string(&self, key: &str, s: &str) -> String {
        if s.is_empty() || KEEP_KEYS.contains(&key) {
            return s.to_owned();
        }
        // Card bindings and similar embed JSON documents in strings.
        if s.starts_with('{')
            && let Ok(mut inner) = serde_json::from_str::<Value>(s)
            && inner.is_object()
        {
            self.anonymise(&mut inner);
            return inner.to_string();
        }
        if s.starts_with("http://") || s.starts_with("https://") || key == "display_url" {
            return self.url(s);
        }
        let key_lower = key.to_ascii_lowercase();
        if TEXT_KEYS.contains(&key)
            || SENSITIVE_KEY_PARTS.iter().any(|p| key_lower.contains(p))
            // Anything with spaces or non-ASCII is human text, whatever its key.
            || s.chars().any(|c| c.is_whitespace() || !c.is_ascii())
        {
            return self.text(s);
        }
        if OPAQUE_KEYS.contains(&key) {
            return WORD.replace_all(s, |c: &Captures<'_>| self.word(&c[0])).into_owned();
        }
        // Anything else is structural; only embedded ids are replaced.
        DIGIT_RUN.replace_all(s, |c: &Captures<'_>| self.word(&c[0])).into_owned()
    }

    /// Human text: URLs and HTML entities are handled separately, every other
    /// word is scrambled.
    fn text(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        let mut protected: Vec<(usize, usize, String)> = URL_IN_TEXT
            .find_iter(s)
            .map(|m| (m.start(), m.end(), self.url(m.as_str())))
            .chain(HTML_ENTITY.find_iter(s).map(|m| (m.start(), m.end(), m.as_str().to_owned())))
            .collect();
        protected.sort_by_key(|p| p.0);
        for (start, end, replacement) in protected {
            if start < last {
                continue;
            }
            out.push_str(&self.words(&s[last..start]));
            out.push_str(&replacement);
            last = end;
        }
        out.push_str(&self.words(&s[last..]));
        out
    }

    fn words(&self, s: &str) -> String {
        WORD.replace_all(s, |c: &Captures<'_>| self.word(&c[0])).into_owned()
    }

    /// Keeps scheme, host, route words and the file extension; scrambles the rest.
    fn url(&self, s: &str) -> String {
        let (scheme, rest) = match s.find("://") {
            Some(ix) => s.split_at(ix + 3),
            None => ("", s),
        };
        let (host, path) = match rest.find(['/', '?']) {
            Some(ix) => rest.split_at(ix),
            None => (rest, ""),
        };
        // Other hosts identify people and projects (`name.github.io`): scramble
        // every label except the top-level domain and `www`.
        let host = if KEEP_HOSTS.contains(&host.to_ascii_lowercase().as_str()) {
            host.to_owned()
        } else {
            let labels: Vec<&str> = host.split('.').collect();
            let last = labels.len().saturating_sub(1);
            labels
                .iter()
                .enumerate()
                .map(
                    |(i, l)| if i == last || *l == "www" { (*l).to_owned() } else { self.words(l) },
                )
                .collect::<Vec<_>>()
                .join(".")
        };
        let path = WORD.replace_all(path, |c: &Captures<'_>| {
            let w = &c[0];
            if ROUTE_WORDS.contains(&w.to_ascii_lowercase().as_str()) {
                w.to_owned()
            } else {
                self.word(w)
            }
        });
        format!("{scheme}{host}{path}")
    }

    /// Replaces each character of `word` with another of the same class,
    /// deterministically for the (case-insensitive) word within this pass.
    fn word(&self, word: &str) -> String {
        let mut hasher = DefaultHasher::new();
        self.key.hash(&mut hasher);
        word.to_lowercase().hash(&mut hasher);
        let mut state = hasher.finish() | 1;
        let mut next = || {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        word.chars()
            .enumerate()
            .map(|(i, c)| {
                let r = next();
                if c.is_ascii_digit() {
                    // Keep ids free of leading zeros.
                    let d = if i == 0 { 1 + r % 9 } else { r % 10 };
                    char::from(b'0' + d as u8)
                } else if c.is_ascii_uppercase() {
                    char::from(b'A' + (r % 26) as u8)
                } else if c.is_ascii_lowercase() {
                    char::from(b'a' + (r % 26) as u8)
                } else if c.is_numeric() {
                    char::from(b'0' + (r % 10) as u8)
                } else {
                    // Other scripts: one code point from a small neutral set.
                    ['ж', 'и', 'л', 'م', 'ن', 'ه', 'あ', 'い', 'う'][(r % 9) as usize]
                }
            })
            .collect()
    }
}

/// Handles and display names in `original` that appear in any string value
/// of `scrubbed` (case-insensitive). Empty means the scrub worked.
///
/// Display names count only on objects that also have a `screen_name`
/// (users and mentions) or a `member_count` (lists), so card names and labels
/// don't raise false alarms.
pub fn leaks(original: &Value, scrubbed: &Value) -> Vec<String> {
    fn identifying(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                let named = map.contains_key("screen_name") || map.contains_key("member_count");
                for (k, v) in map {
                    if let Some(s) = v.as_str()
                        && (k == "screen_name" || (k == "name" && named))
                        && s.chars().count() >= 4
                    {
                        out.push(s.to_lowercase());
                    }
                    identifying(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| identifying(v, out)),
            _ => {}
        }
    }
    fn strings(v: &Value, out: &mut String) {
        match v {
            Value::String(s) => {
                out.push_str(&s.to_lowercase());
                out.push('\u{1}');
            }
            Value::Object(map) => map.values().for_each(|v| strings(v, out)),
            Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
            _ => {}
        }
    }
    let mut wanted = Vec::new();
    identifying(original, &mut wanted);
    wanted.sort();
    wanted.dedup();
    let mut haystack = String::new();
    strings(scrubbed, &mut haystack);
    wanted.into_iter().filter(|w| haystack.contains(w.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "data": {"user": {"result": {
                "__typename": "User",
                "rest_id": "44196397",
                "id": "VXNlcjo0NDE5NjM5Nw==",
                "core": {"name": "Elon Musk", "screen_name": "ElonMusk", "created_at": "Tue Jun 02 20:12:29 +0000 2009"},
                "avatar": {"image_url": "https://pbs.twimg.com/profile_images/2101/AbCdEf_normal.jpg"},
                "relationship_counts": {"followers": 241523482}
            }}},
            "tweet": {
                "rest_id": "2112931457786081428",
                "legacy": {
                    "full_text": "@elonmusk loves #Rust &amp; café 🦀 https://t.co/AbC123xyZ",
                    "entities": {
                        "user_mentions": [{"screen_name": "elonmusk", "indices": [0, 9]}],
                        "hashtags": [{"text": "Rust", "indices": [16, 21]}],
                        "urls": [{"url": "https://t.co/AbC123xyZ", "expanded_url": "https://example.com/elonmusk/post?utm=1", "display_url": "example.com/elonmusk/…"}]
                    },
                    "id_str": "2112931457786081428",
                    "favorite_count": 285637
                }
            },
            "entryId": "tweet-2112931457786081428",
            "cursor": {"cursorType": "Bottom", "value": "DAABCgABG-xyz"},
            "card": {"string_value": "{\"title\":{\"content\":\"Secret Product\"}}"},
            "feedback": {"confirmation": "You'll see fewer posts from ElonMusk."},
            "tipjar": {"cash_app_handle": "elonpays"},
            "link": {"display_url": "manishearth.github.io", "expanded_url": "https://www.manishearth.github.io/blog"}
        })
    }

    fn run() -> (Value, Value) {
        let original = sample();
        let mut v = original.clone();
        Anonymiser::with_key(42).anonymise(&mut v);
        (original, v)
    }

    #[test]
    fn identifying_values_are_gone() {
        let (original, v) = run();
        assert_eq!(leaks(&original, &v), Vec::<String>::new());
        let out = v.to_string().to_lowercase();
        for leaked in [
            "44196397",
            "2112931457786081428",
            "secret product",
            "abc123xyz",
            "abcdef",
            "elonpays",
            "manishearth",
        ] {
            assert!(!out.contains(leaked), "{leaked} survived");
        }
    }

    #[test]
    fn one_word_translations_are_scrambled() {
        // Short ASCII words would otherwise pass as structural values.
        let mut v = json!({"data": {"translation": "Reze", "preview_translation": "Reze",
            "source_language": "ja", "destination_language": "en"}});
        Anonymiser::with_key(42).anonymise(&mut v);
        assert_ne!(v["data"]["translation"], "Reze");
        assert_ne!(v["data"]["preview_translation"], "Reze");
        assert_eq!(v["data"]["translation"].as_str().map(|s| s.chars().count()), Some(4));
        assert_eq!(v["data"]["source_language"], "ja");
        assert_eq!(v["data"]["destination_language"], "en");
    }

    #[test]
    fn structure_numbers_and_dates_are_kept() {
        let (o, v) = run();
        assert_eq!(v["data"]["user"]["result"]["__typename"], "User");
        assert_eq!(
            v["data"]["user"]["result"]["relationship_counts"],
            o["data"]["user"]["result"]["relationship_counts"]
        );
        assert_eq!(
            v["data"]["user"]["result"]["core"]["created_at"],
            o["data"]["user"]["result"]["core"]["created_at"]
        );
        assert_eq!(v["cursor"]["cursorType"], "Bottom");
        assert_eq!(v["tweet"]["legacy"]["entities"]["user_mentions"][0]["indices"], json!([0, 9]));
    }

    #[test]
    fn text_keeps_code_point_length_and_entity_alignment() {
        let (o, v) = run();
        let before = o["tweet"]["legacy"]["full_text"].as_str().unwrap();
        let after = v["tweet"]["legacy"]["full_text"].as_str().unwrap();
        assert_eq!(before.chars().count(), after.chars().count());
        assert!(after.contains("&amp;"), "HTML entities stay intact: {after}");
        assert!(after.contains('🦀'));
        // The mention in the text matches the scrubbed screen_name (case-insensitive).
        let handle =
            v["tweet"]["legacy"]["entities"]["user_mentions"][0]["screen_name"].as_str().unwrap();
        let in_text: String = after.chars().skip(1).take(8).collect();
        assert_eq!(in_text.to_lowercase(), handle.to_lowercase());
        // The hashtag in the text matches the entity.
        let tag = v["tweet"]["legacy"]["entities"]["hashtags"][0]["text"].as_str().unwrap();
        let in_text: String = after.chars().skip(17).take(4).collect();
        assert_eq!(in_text, tag);
        // The t.co URL in the text matches the url entity.
        let url = v["tweet"]["legacy"]["entities"]["urls"][0]["url"].as_str().unwrap();
        assert!(after.ends_with(url), "{after} vs {url}");
    }

    #[test]
    fn ids_are_consistent_across_fields() {
        let (_, v) = run();
        let id = v["tweet"]["legacy"]["id_str"].as_str().unwrap();
        assert_eq!(v["tweet"]["rest_id"], id);
        assert_eq!(v["entryId"], format!("tweet-{id}"));
        assert!(!id.starts_with('0'));
        assert_eq!(id.len(), 19);
    }

    #[test]
    fn urls_keep_x_hosts_routes_and_extension() {
        let (_, v) = run();
        let url = v["data"]["user"]["result"]["avatar"]["image_url"].as_str().unwrap();
        assert!(url.starts_with("https://pbs.twimg.com/profile_images/"), "{url}");
        assert!(url.ends_with("_normal.jpg"), "{url}");
        let other = v["link"]["expanded_url"].as_str().unwrap();
        assert!(other.starts_with("https://www.") && other.contains(".io/"), "{other}");
        assert_eq!(other.len(), "https://www.manishearth.github.io/blog".len());
        let display = v["link"]["display_url"].as_str().unwrap();
        assert!(display.ends_with(".io") && display.len() == "manishearth.github.io".len());
    }

    #[test]
    fn embedded_json_is_scrubbed_and_stays_json() {
        let (_, v) = run();
        let inner: Value =
            serde_json::from_str(v["card"]["string_value"].as_str().unwrap()).unwrap();
        let title = inner["title"]["content"].as_str().unwrap();
        assert_eq!(title.chars().count(), "Secret Product".chars().count());
        assert_ne!(title, "Secret Product");
    }

    #[test]
    fn card_type_names_are_kept() {
        let mut v = json!({"card": {"legacy": {"name": "summary_large_image", "binding_values": [
            {"key": "title", "value": {"string_value": "Private title"}}
        ]}}});
        Anonymiser::with_key(7).anonymise(&mut v);
        assert_eq!(v["card"]["legacy"]["name"], "summary_large_image");
        assert_eq!(v["card"]["legacy"]["binding_values"][0]["key"], "title");
        assert_ne!(
            v["card"]["legacy"]["binding_values"][0]["value"]["string_value"],
            "Private title"
        );
    }

    #[test]
    fn leak_check_ignores_keys_and_non_user_names() {
        let original = json!({"card": {"legacy": {"name": "unified_card"}}, "user": {"name": "Grok", "screen_name": "grok"}});
        let scrubbed = json!({"card": {"legacy": {"name": "unified_card"}}, "user": {"name": "Abcd", "screen_name": "efgh"}, "grok_analysis_button": true});
        assert!(leaks(&original, &scrubbed).is_empty());
        let leaked = json!({"text": "hello grok"});
        assert_eq!(leaks(&original, &leaked), vec!["grok".to_owned()]);
    }

    #[test]
    fn list_names_are_scrubbed_and_checked() {
        let original = json!({"list": {"name": "Rustaceans", "member_count": 3, "mode": "Private",
            "members_context": "3 members", "description": "People who write Rust"}});
        let mut scrubbed = original.clone();
        Anonymiser::with_key(7).anonymise(&mut scrubbed);
        assert_ne!(scrubbed["list"]["name"], "Rustaceans");
        assert_ne!(scrubbed["list"]["description"], "People who write Rust");
        assert_eq!(scrubbed["list"]["mode"], "Private", "structural values are kept");
        assert!(leaks(&original, &scrubbed).is_empty());
        assert_eq!(leaks(&original, &original), vec!["rustaceans".to_owned()]);
    }

    #[test]
    fn different_keys_give_different_output() {
        let mut a = sample();
        let mut b = sample();
        Anonymiser::with_key(1).anonymise(&mut a);
        Anonymiser::with_key(2).anonymise(&mut b);
        assert_ne!(a, b);
    }
}
