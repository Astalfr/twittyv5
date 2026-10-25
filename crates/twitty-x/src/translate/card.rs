//! Cards (`tweet.card`) → link previews and polls.
//!
//! Known card names (2026-10): `summary`, `summary_large_image`,
//! `unified_card` (JSON document in a binding), `<id>:broadcast` (live video),
//! `pollNchoice_text_only` / `pollNchoice_image`. Unknown cards yield `None`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use twitty_model::{Embed, ImageRef, LinkCard, Poll, PollOption};
use url::Url;

use crate::wire::{array_at, at, bool_at, str_at, u64_at};

/// `t.co` → expanded URL, from the tweet's url entities.
pub(crate) type UrlMap = HashMap<String, String>;

/// Translates a tweet's `card`, if it is one Twitty can show.
pub(crate) fn embed(card: &Value, urls: &UrlMap) -> Option<Embed> {
    let legacy = at(card, &["legacy"])?;
    let name = str_at(legacy, &["name"])?;
    let b = Bindings::new(legacy);
    if let Some(n) = name.strip_prefix("poll").and_then(|r| r.split("choice").next()) {
        return poll(&b, n.parse().ok()?).map(Embed::Poll);
    }
    let card_url = b.string("card_url").or_else(|| str_at(legacy, &["url"]));
    let resolve = |u: &str| urls.get(u).map_or(u, String::as_str).to_owned();
    let link = match name {
        "summary" | "summary_large_image" | "player" | "app" => LinkCard {
            url: Url::parse(&resolve(card_url?)).ok()?,
            title: b.string("title")?.to_owned(),
            description: b.string("description").map(str::to_owned),
            image: [
                "photo_image_full_size_large",
                "summary_photo_image_large",
                "thumbnail_image_large",
                "photo_image_full_size",
                "player_image_large",
                "thumbnail_image",
            ]
            .iter()
            .find_map(|k| b.image(k)),
            domain: domain(&b, &resolve(card_url?)),
        },
        "unified_card" => unified(b.string("unified_card")?)?,
        n if n.ends_with(":broadcast") => LinkCard {
            url: Url::parse(b.string("broadcast_url")?).ok()?,
            title: b.string("broadcast_title")?.to_owned(),
            description: b.string("broadcaster_display_name").map(|n| format!("Live · {n}")),
            image: ["broadcast_thumbnail_large", "broadcast_thumbnail"]
                .iter()
                .find_map(|k| b.image(k)),
            domain: "x.com".to_owned(),
        },
        other => {
            tracing::debug!("card {other} not supported");
            return None;
        }
    };
    Some(Embed::Link(link))
}

fn domain(b: &Bindings<'_>, url: &str) -> String {
    b.string("vanity_url")
        .or_else(|| b.string("domain"))
        .map(str::to_owned)
        .or_else(|| Url::parse(url).ok()?.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn poll(b: &Bindings<'_>, n: usize) -> Option<Poll> {
    let options: Vec<PollOption> = (1..=n)
        .filter_map(|i| {
            Some(PollOption {
                label: b.string(&format!("choice{i}_label"))?.to_owned(),
                votes: b
                    .string(&format!("choice{i}_count"))
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0),
            })
        })
        .collect();
    if options.is_empty() {
        return None;
    }
    let ends_at = b
        .string("end_datetime_utc")
        .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
        .map(|d| d.with_timezone(&Utc));
    let closed =
        b.boolean("counts_are_final").unwrap_or(false) || ends_at.is_some_and(|e| e <= Utc::now());
    Some(Poll { options, ends_at, closed })
}

/// `unified_card`: a JSON document of components and destinations.
fn unified(doc: &str) -> Option<LinkCard> {
    let doc: Value = serde_json::from_str(doc).ok()?;
    let components = at(&doc, &["component_objects"])?.as_object()?;
    let details = components.values().find(|c| str_at(c, &["type"]) == Some("details"))?;
    let title = str_at(details, &["data", "title", "content"])?.to_owned();
    let subtitle = str_at(details, &["data", "subtitle", "content"]).map(str::to_owned);
    let destinations = at(&doc, &["destination_objects"])?.as_object()?;
    let dest_key = str_at(details, &["data", "destination"]);
    let dest =
        dest_key.and_then(|k| destinations.get(k)).or_else(|| destinations.values().next())?;
    let url = Url::parse(str_at(dest, &["data", "url_data", "url"])?).ok()?;
    let vanity = str_at(dest, &["data", "url_data", "vanity"]).map(str::to_owned);
    let image = components
        .values()
        .find_map(|c| {
            let id = str_at(c, &["data", "id"])
                .or_else(|| str_at(array_at(c, &["data", "media_list"]).first()?, &["id"]))?;
            at(&doc, &["media_entities", id])
        })
        .and_then(|m| {
            let url = Url::parse(str_at(m, &["media_url_https"])?).ok()?;
            let w = u64_at(m, &["original_info", "width"]).and_then(|w| u32::try_from(w).ok());
            let h = u64_at(m, &["original_info", "height"]).and_then(|h| u32::try_from(h).ok());
            Some(ImageRef::sized(url, w.zip(h)))
        });
    let domain = vanity
        .or(subtitle.clone())
        .or_else(|| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    Some(LinkCard { url, title, description: None, image, domain })
}

/// `binding_values` as a key → value lookup.
struct Bindings<'a> {
    map: HashMap<&'a str, &'a Value>,
}

impl<'a> Bindings<'a> {
    fn new(legacy: &'a Value) -> Self {
        let map = array_at(legacy, &["binding_values"])
            .iter()
            .filter_map(|b| Some((str_at(b, &["key"])?, at(b, &["value"])?)))
            .collect();
        Self { map }
    }

    fn string(&self, key: &str) -> Option<&'a str> {
        str_at(self.map.get(key)?, &["string_value"]).filter(|s| !s.is_empty())
    }

    fn boolean(&self, key: &str) -> Option<bool> {
        bool_at(self.map.get(key)?, &["boolean_value"])
    }

    fn image(&self, key: &str) -> Option<ImageRef> {
        let v = at(self.map.get(key)?, &["image_value"])?;
        let url = Url::parse(str_at(v, &["url"])?).ok()?;
        let w = u64_at(v, &["width"]).and_then(|w| u32::try_from(w).ok());
        let h = u64_at(v, &["height"]).and_then(|h| u32::try_from(h).ok());
        Some(ImageRef::sized(url, w.zip(h)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(name: &str, bindings: Value) -> Value {
        json!({"legacy": {"name": name, "url": "https://t.co/card", "binding_values": bindings}})
    }

    fn s(k: &str, v: &str) -> Value {
        json!({"key": k, "value": {"type": "STRING", "string_value": v}})
    }

    #[test]
    fn summary_large_image() {
        let c = card(
            "summary_large_image",
            json!([
                s("title", "Rust 2027"), s("description", "Edition news"), s("vanity_url", "blog.rust-lang.org"),
                s("card_url", "https://t.co/card"),
                {"key": "photo_image_full_size_large", "value": {"type": "IMAGE", "image_value": {"url": "https://pbs.twimg.com/card_img/1/a", "width": 800, "height": 419}}}
            ]),
        );
        let urls =
            UrlMap::from([("https://t.co/card".into(), "https://blog.rust-lang.org/2027".into())]);
        let Some(Embed::Link(l)) = embed(&c, &urls) else { panic!("link card") };
        assert_eq!(l.url.as_str(), "https://blog.rust-lang.org/2027");
        assert_eq!(l.title, "Rust 2027");
        assert_eq!(l.domain, "blog.rust-lang.org");
        assert_eq!(l.image.unwrap().size, Some((800, 419)));
    }

    #[test]
    fn summary_without_url_map_uses_tco_and_host() {
        let c = card("summary", json!([s("title", "T"), s("card_url", "https://t.co/card")]));
        let Some(Embed::Link(l)) = embed(&c, &UrlMap::new()) else { panic!() };
        assert_eq!(l.url.as_str(), "https://t.co/card");
        assert_eq!(l.domain, "t.co");
    }

    #[test]
    fn polls() {
        let c = card(
            "poll3choice_text_only",
            json!([
                s("choice1_label", "GPUI"), s("choice1_count", "120"),
                s("choice2_label", "egui"), s("choice2_count", "80"),
                s("choice3_label", "Slint"), s("choice3_count", "nope"),
                s("end_datetime_utc", "2099-01-01T00:00:00Z"),
                {"key": "counts_are_final", "value": {"type": "BOOLEAN", "boolean_value": false}}
            ]),
        );
        let Some(Embed::Poll(p)) = embed(&c, &UrlMap::new()) else { panic!("poll") };
        assert_eq!(p.options.len(), 3);
        assert_eq!(p.options[0].votes, 120);
        assert_eq!(p.options[2].votes, 0, "unparsable count is zero");
        assert!(!p.closed);
        let ended = card(
            "poll2choice_text_only",
            json!([
                s("choice1_label", "a"),
                s("choice2_label", "b"),
                s("end_datetime_utc", "2000-01-01T00:00:00Z")
            ]),
        );
        let Some(Embed::Poll(p)) = embed(&ended, &UrlMap::new()) else { panic!() };
        assert!(p.closed, "past end date");
    }

    #[test]
    fn unified_card_reads_details_destination_and_media() {
        let doc = json!({
            "type": "video_website",
            "component_objects": {
                "details_1": {"type": "details", "data": {"title": {"content": "Game"}, "subtitle": {"content": "game.com"}, "destination": "d1"}},
                "media_1": {"type": "media", "data": {"id": "13_1"}}
            },
            "destination_objects": {"d1": {"type": "browser", "data": {"url_data": {"url": "https://game.com/play", "vanity": "game.com"}}}},
            "media_entities": {"13_1": {"media_url_https": "https://pbs.twimg.com/m.jpg", "original_info": {"width": 1280, "height": 720}}}
        });
        let c = card("unified_card", json!([s("unified_card", &doc.to_string())]));
        let Some(Embed::Link(l)) = embed(&c, &UrlMap::new()) else { panic!() };
        assert_eq!(l.title, "Game");
        assert_eq!(l.url.as_str(), "https://game.com/play");
        assert_eq!(l.domain, "game.com");
        assert_eq!(l.image.unwrap().size, Some((1280, 720)));
    }

    #[test]
    fn broadcast() {
        let c = card(
            "745291183405076480:broadcast",
            json!([
                s("broadcast_url", "https://x.com/i/broadcasts/1"),
                s("broadcast_title", "Launch"),
                s("broadcaster_display_name", "NASA")
            ]),
        );
        let Some(Embed::Link(l)) = embed(&c, &UrlMap::new()) else { panic!() };
        assert_eq!((l.title.as_str(), l.description.as_deref()), ("Launch", Some("Live · NASA")));
    }

    #[test]
    fn unknown_or_incomplete_cards_are_none() {
        assert!(embed(&card("promo_image_convo", json!([])), &UrlMap::new()).is_none());
        assert!(
            embed(&card("summary", json!([s("card_url", "https://t.co/x")])), &UrlMap::new())
                .is_none(),
            "no title"
        );
        assert!(
            embed(&json!({"rest_id": "https://t.co/x"}), &UrlMap::new()).is_none(),
            "no legacy"
        );
        assert!(
            embed(&card("unified_card", json!([s("unified_card", "not json")])), &UrlMap::new())
                .is_none()
        );
    }
}
