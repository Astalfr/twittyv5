//! `extended_entities.media` → `Media`.

use std::time::Duration;

use serde_json::Value;
use twitty_model::{ImageRef, Media};
use url::Url;

use crate::wire::{array_at, at, str_at, u64_at};

/// The media attached to a tweet (`legacy`), in order.
pub(crate) fn media(legacy: &Value) -> Vec<Media> {
    let list = match at(legacy, &["extended_entities", "media"]) {
        Some(m) => array_at(m, &[]),
        None => array_at(legacy, &["entities", "media"]),
    };
    list.iter().filter_map(one).collect()
}

fn one(m: &Value) -> Option<Media> {
    let preview = image(m)?;
    let alt = str_at(m, &["ext_alt_text"]).filter(|a| !a.is_empty()).map(str::to_owned);
    Some(match str_at(m, &["type"])? {
        "photo" => Media::Image { image: preview, alt },
        "video" => Media::Video {
            poster: preview,
            stream: best_mp4(m),
            duration: u64_at(m, &["video_info", "duration_millis"]).map(Duration::from_millis),
            alt,
        },
        "animated_gif" => Media::Gif { poster: preview, stream: best_mp4(m), alt },
        other => {
            tracing::debug!("unknown media type {other}, shown as image");
            Media::Image { image: preview, alt }
        }
    })
}

/// The still image, sized for a column (`?name=medium`, at most 1200 px).
fn image(m: &Value) -> Option<ImageRef> {
    let base = str_at(m, &["media_url_https"])?;
    let url = if base.contains('?') { base.to_owned() } else { format!("{base}?name=medium") };
    let w = u64_at(m, &["original_info", "width"]).and_then(|w| u32::try_from(w).ok());
    let h = u64_at(m, &["original_info", "height"]).and_then(|h| u32::try_from(h).ok());
    Some(ImageRef { url: Url::parse(&url).ok()?, size: w.zip(h) })
}

/// The highest-bitrate MP4 variant.
fn best_mp4(m: &Value) -> Option<Url> {
    array_at(m, &["video_info", "variants"])
        .iter()
        .filter(|v| str_at(v, &["content_type"]) == Some("video/mp4"))
        .max_by_key(|v| u64_at(v, &["bitrate"]).unwrap_or(0))
        .and_then(|v| Url::parse(str_at(v, &["url"])?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn photo_video_and_gif() {
        let legacy = json!({"extended_entities": {"media": [
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/A.jpg", "ext_alt_text": "a cat",
             "original_info": {"width": 1200, "height": 800}},
            {"type": "video", "media_url_https": "https://pbs.twimg.com/amplify_video_thumb/1/img/B.jpg",
             "original_info": {"width": 1920, "height": 1080},
             "video_info": {"duration_millis": 43119, "variants": [
                {"content_type": "application/x-mpegURL", "url": "https://video.twimg.com/x.m3u8"},
                {"content_type": "video/mp4", "bitrate": 256000, "url": "https://video.twimg.com/low.mp4"},
                {"content_type": "video/mp4", "bitrate": 2176000, "url": "https://video.twimg.com/high.mp4"}
             ]}},
            {"type": "animated_gif", "media_url_https": "https://pbs.twimg.com/tweet_video_thumb/C.jpg",
             "video_info": {"variants": [{"content_type": "video/mp4", "bitrate": 0, "url": "https://video.twimg.com/c.mp4"}]}}
        ]}});
        let m = media(&legacy);
        assert_eq!(m.len(), 3);
        let Media::Image { image, alt } = &m[0] else { panic!("photo") };
        assert_eq!(image.url.as_str(), "https://pbs.twimg.com/media/A.jpg?name=medium");
        assert_eq!(image.size, Some((1200, 800)));
        assert_eq!(alt.as_deref(), Some("a cat"));
        let Media::Video { stream, duration, .. } = &m[1] else { panic!("video") };
        assert_eq!(stream.as_ref().unwrap().as_str(), "https://video.twimg.com/high.mp4");
        assert_eq!(*duration, Some(Duration::from_millis(43119)));
        assert!(matches!(&m[2], Media::Gif { stream: Some(_), .. }));
    }

    #[test]
    fn falls_back_to_entities_and_skips_broken_items() {
        let legacy = json!({"entities": {"media": [
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/A.jpg"},
            {"type": "photo"}
        ]}});
        assert_eq!(media(&legacy).len(), 1);
        assert!(media(&json!({})).is_empty());
    }
}
