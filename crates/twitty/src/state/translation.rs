//! Machine-translated tweets: which text a row shows, and the note under it.
//!
//! A translated tweet shows its translation by default; the viewer can switch
//! each tweet back to the original, and every column follows that choice.

use std::{borrow::Cow, collections::HashSet};

use twitty_model::{Post, PostId, RichText};

/// Tweets the viewer switched back to their original text.
#[derive(Debug, Clone, Default)]
pub struct Originals(HashSet<PostId>);

impl Originals {
    pub fn showing_original(&self, id: &PostId) -> bool {
        self.0.contains(id)
    }

    /// Switches `id` between its translation and its original text.
    pub fn toggle(&mut self, id: &PostId) {
        if !self.0.remove(id) {
            self.0.insert(id.clone());
        }
    }
}

/// The text to render for a tweet, and the note explaining it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown<'a> {
    pub body: &'a RichText,
    pub note: Option<Note>,
}

/// The line above a translated tweet's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// "Translated from Japanese", or "Original in Japanese".
    pub label: String,
    /// The switch to the other text: "Show original" / "Show translation".
    pub action: &'static str,
}

/// What to show for `post`, given whether the viewer asked for the original.
pub fn shown(post: &Post, showing_original: bool) -> Shown<'_> {
    let Some(tr) = &post.translation else {
        return Shown { body: &post.body, note: None };
    };
    let from = tr.from.as_deref().or(post.lang.as_deref()).map(language_name);
    if showing_original {
        let label = from.map_or_else(|| "Original text".to_owned(), |l| format!("Original in {l}"));
        Shown { body: &post.body, note: Some(Note { label, action: "Show translation" }) }
    } else {
        let label =
            from.map_or_else(|| "Translated".to_owned(), |l| format!("Translated from {l}"));
        Shown { body: &tr.body, note: Some(Note { label, action: "Show original" }) }
    }
}

/// English name of a BCP 47 language code; unknown codes are shown as is.
pub fn language_name(code: &str) -> Cow<'static, str> {
    let lower = code.to_ascii_lowercase();
    let name = match lower.as_str() {
        "zh-tw" | "zh-hant" => Some("Traditional Chinese"),
        "zh-cn" | "zh-hans" => Some("Simplified Chinese"),
        "pt-br" => Some("Brazilian Portuguese"),
        _ => None,
    };
    let base = lower.split(['-', '_']).next().unwrap_or_default();
    let name = name.or(match base {
        "ar" => Some("Arabic"),
        "bg" => Some("Bulgarian"),
        "bn" => Some("Bengali"),
        "ca" => Some("Catalan"),
        "cs" => Some("Czech"),
        "da" => Some("Danish"),
        "de" => Some("German"),
        "el" => Some("Greek"),
        "en" => Some("English"),
        "es" => Some("Spanish"),
        "et" => Some("Estonian"),
        "eu" => Some("Basque"),
        "fa" => Some("Persian"),
        "fi" => Some("Finnish"),
        "fil" | "tl" => Some("Filipino"),
        "fr" => Some("French"),
        "ga" => Some("Irish"),
        "gl" => Some("Galician"),
        "gu" => Some("Gujarati"),
        "he" | "iw" => Some("Hebrew"),
        "hi" => Some("Hindi"),
        "hr" => Some("Croatian"),
        "hu" => Some("Hungarian"),
        "hy" => Some("Armenian"),
        "id" | "in" => Some("Indonesian"),
        "is" => Some("Icelandic"),
        "it" => Some("Italian"),
        "ja" => Some("Japanese"),
        "ka" => Some("Georgian"),
        "kn" => Some("Kannada"),
        "ko" => Some("Korean"),
        "lt" => Some("Lithuanian"),
        "lv" => Some("Latvian"),
        "mr" => Some("Marathi"),
        "ms" => Some("Malay"),
        "nl" => Some("Dutch"),
        "no" | "nb" => Some("Norwegian"),
        "pa" => Some("Punjabi"),
        "pl" => Some("Polish"),
        "pt" => Some("Portuguese"),
        "ro" => Some("Romanian"),
        "ru" => Some("Russian"),
        "sk" => Some("Slovak"),
        "sl" => Some("Slovenian"),
        "sr" => Some("Serbian"),
        "sv" => Some("Swedish"),
        "ta" => Some("Tamil"),
        "te" => Some("Telugu"),
        "th" => Some("Thai"),
        "tr" => Some("Turkish"),
        "uk" => Some("Ukrainian"),
        "ur" => Some("Urdu"),
        "vi" => Some("Vietnamese"),
        "zh" => Some("Chinese"),
        _ => None,
    });
    match name {
        Some(n) => Cow::Borrowed(n),
        None => Cow::Owned(code.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{Backend, FeedSource, PageRequest, Translation, mock::MockBackend};

    fn post(translation: Option<Translation>) -> Post {
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(MockBackend::new().feed(&FeedSource::Bookmarks, PageRequest::First))
            .unwrap();
        let mut post =
            page.items.iter().find_map(|i| i.posts().first().map(|p| (*p).clone())).unwrap();
        post.body = RichText::plain("こんにちは");
        post.lang = Some("ja".into());
        post.translation = translation;
        post
    }

    fn translated(from: Option<&str>) -> Translation {
        Translation { body: RichText::plain("Hello"), from: from.map(Into::into), to: None }
    }

    #[test]
    fn a_post_without_translation_shows_its_body_and_no_note() {
        let p = post(None);
        assert_eq!(shown(&p, false), Shown { body: &p.body, note: None });
        assert_eq!(shown(&p, true), Shown { body: &p.body, note: None });
    }

    #[test]
    fn a_translated_post_shows_the_translation_until_asked_for_the_original() {
        let p = post(Some(translated(Some("ja"))));
        let s = shown(&p, false);
        assert_eq!(s.body.text(), "Hello");
        assert_eq!(
            s.note,
            Some(Note { label: "Translated from Japanese".into(), action: "Show original" })
        );
        let s = shown(&p, true);
        assert_eq!(s.body.text(), "こんにちは");
        assert_eq!(
            s.note,
            Some(Note { label: "Original in Japanese".into(), action: "Show translation" })
        );
    }

    #[test]
    fn the_source_language_falls_back_to_the_posts_language() {
        let p = post(Some(translated(None)));
        assert_eq!(shown(&p, false).note.unwrap().label, "Translated from Japanese");
        let mut p = post(Some(translated(None)));
        p.lang = None;
        assert_eq!(shown(&p, false).note.unwrap().label, "Translated");
        assert_eq!(shown(&p, true).note.unwrap().label, "Original text");
    }

    #[test]
    fn originals_toggle_per_post() {
        let mut o = Originals::default();
        let (a, b) = (PostId::new("a"), PostId::new("b"));
        assert!(!o.showing_original(&a));
        o.toggle(&a);
        assert!(o.showing_original(&a));
        assert!(!o.showing_original(&b));
        o.toggle(&a);
        assert!(!o.showing_original(&a));
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name("ja"), "Japanese");
        assert_eq!(language_name("FR"), "French");
        assert_eq!(language_name("pt-BR"), "Brazilian Portuguese");
        assert_eq!(language_name("pt-PT"), "Portuguese");
        assert_eq!(language_name("zh-TW"), "Traditional Chinese");
        assert_eq!(language_name("zh"), "Chinese");
        assert_eq!(language_name("qaa"), "qaa");
        assert_eq!(language_name(""), "");
    }
}
