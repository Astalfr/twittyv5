//! `extended_entities.media` → `Media`.

use std::time::Duration;

use serde_json::Value;
use twitty_model::{ImageRef, Media, Region, VideoVariant};
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
            variants: variants(m),
            duration: u64_at(m, &["video_info", "duration_millis"]).map(Duration::from_millis),
            alt,
        },
        "animated_gif" => Media::Gif {
            poster: preview,
            stream: variants(m).into_iter().next().map(|v| v.url),
            alt,
        },
        other => {
            tracing::debug!("unknown media type {other}, shown as image");
            Media::Image { image: preview, alt }
        }
    })
}

/// The still image, sized for a column (`?name=medium`, at most 1200 px),
/// with Twitter's suggested crops (`focus_rects`, one per tile shape, in
/// pixels of the original). Videos and GIFs have none.
fn image(m: &Value) -> Option<ImageRef> {
    let base = str_at(m, &["media_url_https"])?;
    let url = if base.contains('?') { base.to_owned() } else { format!("{base}?name=medium") };
    let w = u64_at(m, &["original_info", "width"]).and_then(|w| u32::try_from(w).ok());
    let h = u64_at(m, &["original_info", "height"]).and_then(|h| u32::try_from(h).ok());
    let crops = array_at(m, &["original_info", "focus_rects"]).iter().filter_map(|r| {
        let side = |k: &str| u64_at(r, &[k]).and_then(|v| u32::try_from(v).ok());
        Some(Region { x: side("x")?, y: side("y")?, w: side("w")?, h: side("h")? })
    });
    Some(ImageRef::sized(Url::parse(&url).ok()?, w.zip(h)).with_crops(crops))
}

/// The MP4 variants, highest bitrate first (HLS playlists are left out).
fn variants(m: &Value) -> Vec<VideoVariant> {
    let mut variants: Vec<VideoVariant> = array_at(m, &["video_info", "variants"])
        .iter()
        .filter(|v| str_at(v, &["content_type"]) == Some("video/mp4"))
        .filter_map(|v| {
            let url = Url::parse(str_at(v, &["url"])?).ok()?;
            let bitrate = u64_at(v, &["bitrate"]).and_then(|b| u32::try_from(b).ok());
            Some(VideoVariant { size: size_in_path(&url), url, bitrate })
        })
        .collect();
    variants.sort_by_key(|v| std::cmp::Reverse(v.bitrate.unwrap_or(0)));
    variants.dedup_by(|a, b| a.url == b.url);
    variants
}

/// The `WIDTHxHEIGHT` segment of a Twitter video path (`/vid/avc1/1280x720/…`).
fn size_in_path(url: &Url) -> Option<(u32, u32)> {
    url.path_segments()?.find_map(|segment| {
        let (w, h) = segment.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?)).filter(|&(w, h): &(u32, u32)| w > 0 && h > 0)
    })
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
                {"content_type": "video/mp4", "bitrate": 256000, "url": "https://video.twimg.com/vid/avc1/480x270/low.mp4"},
                {"content_type": "video/mp4", "bitrate": 2176000, "url": "https://video.twimg.com/vid/avc1/1280x720/high.mp4"}
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
        let Media::Video { variants, duration, .. } = &m[1] else { panic!("video") };
        let urls: Vec<&str> = variants.iter().map(|v| v.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://video.twimg.com/vid/avc1/1280x720/high.mp4",
                "https://video.twimg.com/vid/avc1/480x270/low.mp4"
            ],
            "every MP4, best first, no playlist"
        );
        assert_eq!((variants[0].bitrate, variants[0].size), (Some(2_176_000), Some((1280, 720))));
        assert_eq!(m[1].stream(), Some(&variants[0].url));
        assert_eq!(*duration, Some(Duration::from_millis(43119)));
        assert!(matches!(&m[2], Media::Gif { stream: Some(_), .. }));
    }

    #[test]
    fn a_photo_keeps_the_crops_that_fit_it() {
        let legacy = json!({"extended_entities": {"media": [
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/A.jpg",
             "original_info": {"width": 1075, "height": 1217, "focus_rects": [
                {"x": 0, "y": 0, "w": 1075, "h": 602},
                {"x": 151, "y": 0, "w": 609, "h": 1217},
                {"x": 600, "y": 0, "w": 609, "h": 1217},
                {"x": 0, "y": 0, "w": 0, "h": 10},
                {"x": 0, "y": "?", "w": 10, "h": 10}
             ]}},
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/B.jpg",
             "original_info": {"focus_rects": [{"x": 0, "y": 0, "w": 10, "h": 10}]}}
        ]}});
        let m = media(&legacy);
        let r = |x, y, w, h| Region { x, y, w, h };
        assert_eq!(m[0].preview().crops, [r(0, 0, 1075, 602), r(151, 0, 609, 1217)]);
        assert!(m[1].preview().crops.is_empty(), "no size to check them against");
    }

    #[test]
    fn a_variant_without_a_size_in_its_path_has_none() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert_eq!(
            size_in_path(&url("https://video.twimg.com/vid/avc1/720x1280/a.mp4")),
            Some((720, 1280))
        );
        assert_eq!(size_in_path(&url("https://video.twimg.com/vid/avc1/511d317/a.mp4")), None);
        assert_eq!(size_in_path(&url("https://video.twimg.com/0x0/a.mp4")), None);
        assert_eq!(size_in_path(&url("https://video.twimg.com/tweet_video/x.mp4")), None);
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
