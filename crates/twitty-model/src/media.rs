use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

/// A remote image with optional pixel dimensions, used to reserve layout space
/// before the image loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    /// Where to fetch the image.
    pub url: Url,
    /// Intrinsic size in pixels, when known.
    pub size: Option<(u32, u32)>,
    /// Regions worth keeping when the image is cropped to a box of another
    /// shape, one per box shape the source suggests, in pixels of `size`.
    /// Empty when there are none: crop around the centre.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crops: Vec<Region>,
}

impl ImageRef {
    /// An image without size information.
    pub fn new(url: Url) -> Self {
        Self { url, size: None, crops: Vec::new() }
    }

    /// An image of known size, without suggested crops.
    pub fn sized(url: Url, size: Option<(u32, u32)>) -> Self {
        Self { url, size, crops: Vec::new() }
    }

    /// Adds the suggested `crops` that are non-empty and inside the image.
    /// Without a size, none can be checked, so none are kept.
    pub fn with_crops(mut self, crops: impl IntoIterator<Item = Region>) -> Self {
        if let Some(size) = self.size {
            self.crops.extend(crops.into_iter().filter(|r| r.fits_in(size)));
        }
        self
    }

    /// Width / height, when known and non-degenerate.
    pub fn aspect(&self) -> Option<AspectRatio> {
        self.size.and_then(|(w, h)| AspectRatio::new(w, h))
    }
}

/// A rectangle in an image, in pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    /// Whether the region is non-empty and inside a `size` (width, height) image.
    pub fn fits_in(&self, (width, height): (u32, u32)) -> bool {
        self.w > 0
            && self.h > 0
            && self.x.checked_add(self.w).is_some_and(|r| r <= width)
            && self.y.checked_add(self.h).is_some_and(|b| b <= height)
    }
}

/// A width:height ratio with both sides non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AspectRatio {
    width: u32,
    height: u32,
}

impl AspectRatio {
    /// Returns `None` if either side is zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    /// Width divided by height.
    pub fn as_f32(self) -> f32 {
        self.width as f32 / self.height as f32
    }
}

/// One playable file of a video, at one quality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoVariant {
    pub url: Url,
    /// Bits per second, when known.
    pub bitrate: Option<u32>,
    /// Width and height in pixels, when known.
    pub size: Option<(u32, u32)>,
}

/// Media attached to a tweet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Media {
    /// A still image.
    Image { image: ImageRef, alt: Option<String> },
    /// A video. `variants` are its playable files, best quality first
    /// (empty when none can be played).
    Video {
        poster: ImageRef,
        #[serde(default)]
        variants: Vec<VideoVariant>,
        duration: Option<Duration>,
        alt: Option<String>,
    },
    /// A looping animation without sound.
    Gif { poster: ImageRef, stream: Option<Url>, alt: Option<String> },
}

impl Media {
    /// The still image to show for this media.
    pub fn preview(&self) -> &ImageRef {
        match self {
            Media::Image { image, .. } => image,
            Media::Video { poster, .. } | Media::Gif { poster, .. } => poster,
        }
    }

    /// The file to play by default: a video's best variant, a GIF's stream.
    pub fn stream(&self) -> Option<&Url> {
        match self {
            Media::Image { .. } => None,
            Media::Video { variants, .. } => variants.first().map(|v| &v.url),
            Media::Gif { stream, .. } => stream.as_ref(),
        }
    }

    /// Alternative text, if provided.
    pub fn alt(&self) -> Option<&str> {
        match self {
            Media::Image { alt, .. } | Media::Video { alt, .. } | Media::Gif { alt, .. } => {
                alt.as_deref()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_rejects_zero_sides() {
        assert!(AspectRatio::new(0, 10).is_none());
        assert!(AspectRatio::new(10, 0).is_none());
        assert!((AspectRatio::new(16, 9).unwrap().as_f32() - 1.777).abs() < 0.01);
    }

    #[test]
    fn only_crops_inside_the_image_are_kept() {
        let url = Url::parse("https://img.example/p.jpg").unwrap();
        let r = |x, y, w, h| Region { x, y, w, h };
        let crops = [r(0, 0, 100, 50), r(0, 0, 0, 50), r(50, 0, 51, 50), r(0, 1, 100, 50)];
        let image = ImageRef::sized(url.clone(), Some((100, 50))).with_crops(crops);
        assert_eq!(image.crops, [r(0, 0, 100, 50)]);
        assert!(ImageRef::new(url).with_crops(crops).crops.is_empty(), "no size, no check");
        assert!(!r(u32::MAX, 0, 2, 1).fits_in((u32::MAX, 1)), "no overflow");
    }

    #[test]
    fn preview_returns_poster_for_video() {
        let poster = ImageRef::new(Url::parse("https://img.example/p.jpg").unwrap());
        let m = Media::Video {
            poster: poster.clone(),
            variants: Vec::new(),
            duration: None,
            alt: None,
        };
        assert_eq!(m.preview(), &poster);
        assert_eq!(m.stream(), None);
    }

    #[test]
    fn a_videos_stream_is_its_best_variant() {
        let url = |s: &str| Url::parse(s).unwrap();
        let variant = |s: &str| VideoVariant { url: url(s), bitrate: None, size: None };
        let m = Media::Video {
            poster: ImageRef::new(url("https://img.example/p.jpg")),
            variants: vec![
                variant("https://v.example/high.mp4"),
                variant("https://v.example/low.mp4"),
            ],
            duration: None,
            alt: None,
        };
        assert_eq!(m.stream().map(Url::as_str), Some("https://v.example/high.mp4"));
    }
}
