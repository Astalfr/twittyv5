//! Sizes for shrinking remote images before GPUI draws them.
//!
//! GPUI uploads a bitmap as a texture with no mipmaps and samples it
//! bilinearly. Drawing one at well under its pixel size skips source pixels,
//! so a 1200 px photo in a 400 px tile comes out jagged. Remote images are
//! instead resampled on the CPU to about the size they are drawn at, which
//! leaves GPUI at most a small shrink.

/// Ratio between neighbouring box buckets. It is also the most GPUI is left to
/// shrink an image by in each of the two steps (bucket and resample).
pub const STEP: f32 = 1.25;

/// The smallest bucket, in device pixels.
const MIN_BUCKET: f32 = 16.0;

/// The largest bucket, in device pixels: no image is resampled to more.
const MAX_BUCKET: f32 = 8192.0;

/// A box in device pixels, each side rounded up to a bucket.
///
/// Buckets keep the number of resampled copies small: a column resized by a
/// few pixels keeps the image it has instead of loading a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoxPx {
    pub w: u32,
    pub h: u32,
}

impl BoxPx {
    /// The bucket for a `w` × `h` box in logical pixels drawn at `scale`
    /// (the window's scale factor). `None` for an empty or invalid box.
    pub fn bucket(w: f32, h: f32, scale: f32) -> Option<Self> {
        Some(Self { w: bucket(w * scale)?, h: bucket(h * scale)? })
    }
}

/// The smallest `MIN_BUCKET × STEPⁿ` at or above `device_px`.
fn bucket(device_px: f32) -> Option<u32> {
    if !(device_px.is_finite() && device_px > 0.0) {
        return None;
    }
    let mut b = MIN_BUCKET;
    while b < device_px && b < MAX_BUCKET {
        b *= STEP;
    }
    // Truncation is fine: `b` is a positive whole number of pixels at most 8192.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0 < b ≤ 8192")]
    Some(b.min(MAX_BUCKET).ceil() as u32)
}

/// The size to resample a `source` (width, height) bitmap to so it just covers
/// `target`, keeping its aspect ratio. `None` when it is already small enough
/// for GPUI to shrink cleanly (or is empty); images are never enlarged.
pub fn cover_size(source: (u32, u32), target: BoxPx) -> Option<(u32, u32)> {
    let (sw, sh) = source;
    if sw == 0 || sh == 0 {
        return None;
    }
    let (sw, sh) = (f64::from(sw), f64::from(sh));
    let scale = (f64::from(target.w) / sw).max(f64::from(target.h) / sh);
    if scale >= 1.0 / f64::from(STEP) {
        return None;
    }
    // Both sides are at most the source's (scale < 1), so they fit in u32.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "≤ source size")]
    let side = |s: f64| (s * scale).ceil().max(1.0) as u32;
    Some((side(sw), side(sh)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn buckets_round_up_by_steps() {
        assert_eq!(bucket(1.0), Some(16));
        assert_eq!(bucket(16.0), Some(16));
        assert_eq!(bucket(17.0), Some(20));
        assert_eq!(bucket(20.5), Some(25));
        assert_eq!(BoxPx::bucket(100.0, 50.0, 2.0), Some(BoxPx { w: 233, h: 120 }));
    }

    #[test]
    fn empty_or_invalid_boxes_have_no_bucket() {
        assert_eq!(BoxPx::bucket(0.0, 10.0, 1.0), None);
        assert_eq!(BoxPx::bucket(10.0, -1.0, 1.0), None);
        assert_eq!(BoxPx::bucket(f32::NAN, 10.0, 1.0), None);
        assert_eq!(BoxPx::bucket(10.0, f32::INFINITY, 1.0), None);
    }

    #[test]
    fn a_large_photo_in_a_small_tile_is_shrunk_to_cover_it() {
        // A 3:2 photo in a 2:1 tile: the width decides.
        let target = BoxPx { w: 400, h: 200 };
        assert_eq!(cover_size((1200, 800), target), Some((400, 267)));
        // A tall photo in the same tile.
        assert_eq!(cover_size((800, 1200), target), Some((400, 600)));
    }

    #[test]
    fn small_or_slightly_large_images_are_left_alone() {
        let target = BoxPx { w: 400, h: 300 };
        assert_eq!(cover_size((300, 200), target), None, "never enlarged");
        assert_eq!(cover_size((480, 360), target), None, "GPUI shrinks 1.2× cleanly");
        assert_eq!(cover_size((0, 360), target), None);
    }

    proptest! {
        #[test]
        fn a_bucket_is_at_least_the_box_and_at_most_a_step_more(px in 0.01f32..8000.0) {
            let b = bucket(px).unwrap() as f32;
            prop_assert!(b >= px);
            prop_assert!(b <= (px * STEP).max(MIN_BUCKET).ceil());
        }

        #[test]
        fn a_resampled_image_covers_the_box_and_keeps_its_shape(
            sw in 1u32..5000, sh in 1u32..5000, tw in 1u32..3000, th in 1u32..3000,
        ) {
            let target = BoxPx { w: tw, h: th };
            if let Some((w, h)) = cover_size((sw, sh), target) {
                prop_assert!(w >= tw && h >= th, "covers {tw}×{th}: {w}×{h}");
                prop_assert!(w <= sw && h <= sh, "never enlarged");
                // Each side is rounded up by under a pixel, so the aspect
                // ratio drifts by less than one pixel's worth on either side.
                let skew = (u64::from(w) * u64::from(sh)).abs_diff(u64::from(h) * u64::from(sw));
                prop_assert!(skew <= u64::from(sw) + u64::from(sh), "{w}×{h} from {sw}×{sh}");
            }
        }
    }
}
