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
}

impl ImageRef {
    /// An image without size information.
    pub fn new(url: Url) -> Self {
        Self { url, size: None }
    }

    /// Width / height, when known and non-degenerate.
    pub fn aspect(&self) -> Option<AspectRatio> {
        self.size.and_then(|(w, h)| AspectRatio::new(w, h))
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

/// Media attached to a post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Media {
    /// A still image.
    Image { image: ImageRef, alt: Option<String> },
    /// A video; `stream` is the best playable file, if any.
    Video { poster: ImageRef, stream: Option<Url>, duration: Option<Duration>, alt: Option<String> },
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
    fn preview_returns_poster_for_video() {
        let poster = ImageRef::new(Url::parse("https://img.example/p.jpg").unwrap());
        let m = Media::Video { poster: poster.clone(), stream: None, duration: None, alt: None };
        assert_eq!(m.preview(), &poster);
    }
}
