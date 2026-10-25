//! Geometry of the 1–4 item media grid, in the style of Twitter.

use serde::{Deserialize, Serialize};
use twitty_model::{AspectRatio, ImageRef};

/// Gap between tiles, in pixels.
pub const GAP: f32 = 2.0;

/// The tallest a media grid may be (Settings → Timelines), in logical
/// pixels, always within [`Self::MIN`]..=[`Self::MAX`]. No cap is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u16", into = "u16")]
pub struct MaxHeight(u16);

impl MaxHeight {
    pub const MIN: u16 = 150;
    pub const MAX: u16 = 800;
    /// Slider granularity.
    pub const STEP: u16 = 10;

    /// `px` rounded to the nearest pixel and clamped to the allowed range.
    pub fn new(px: f32) -> Self {
        let px = if px.is_finite() { px.round() } else { 0.0 };
        // In range after the clamp, so the cast is exact.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped")]
        Self(px.clamp(f32::from(Self::MIN), f32::from(Self::MAX)) as u16)
    }

    /// Height in logical pixels.
    pub fn px(self) -> f32 {
        f32::from(self.0)
    }

    /// The settings slider's position for `limit`. The slider goes one step
    /// past [`Self::MAX`], and that last position means no cap.
    pub fn slider_value(limit: Option<Self>) -> f32 {
        limit.map_or(f32::from(Self::MAX + Self::STEP), Self::px)
    }

    /// The cap at slider position `value` (see [`Self::slider_value`]).
    pub fn from_slider(value: f32) -> Option<Self> {
        (value < f32::from(Self::MAX) + f32::from(Self::STEP) / 2.0).then(|| Self::new(value))
    }

    /// The cap as the settings page shows it.
    pub fn label(limit: Option<Self>) -> String {
        limit.map_or_else(|| "No limit".to_owned(), |l| format!("{} px", l.0))
    }
}

impl From<u16> for MaxHeight {
    fn from(px: u16) -> Self {
        Self(px.clamp(Self::MIN, Self::MAX))
    }
}

impl From<MaxHeight> for u16 {
    fn from(limit: MaxHeight) -> Self {
        limit.0
    }
}

/// A tile rectangle relative to the grid's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The grid's tiles and total height.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub tiles: Vec<Tile>,
    pub height: f32,
}

/// Which corners of a tile touch a corner of the grid and must be rounded.
///
/// GPUI clips children to a rectangle, not to the parent's rounded corners,
/// so each image has to round its own outer corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Corners {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
}

impl Corners {
    /// All four corners rounded.
    pub const ALL: Corners =
        Corners { top_left: true, top_right: true, bottom_left: true, bottom_right: true };
    /// Only the top corners (an image above other content).
    pub const TOP: Corners =
        Corners { top_left: true, top_right: true, bottom_left: false, bottom_right: false };
    /// Only the bottom corners (an image below other content).
    pub const BOTTOM: Corners =
        Corners { top_left: false, top_right: false, bottom_left: true, bottom_right: true };
}

impl Grid {
    /// The outer corners of `tile`, for a grid `width` pixels wide.
    pub fn corners(&self, tile: &Tile, width: f32) -> Corners {
        const EPS: f32 = 0.5;
        let left = tile.x <= EPS;
        let top = tile.y <= EPS;
        let right = tile.x + tile.w >= width - EPS;
        let bottom = tile.y + tile.h >= self.height - EPS;
        Corners {
            top_left: top && left,
            top_right: top && right,
            bottom_left: bottom && left,
            bottom_right: bottom && right,
        }
    }
}

/// Lays out `count` media tiles in a grid of `width` pixels.
///
/// A single item keeps its aspect ratio, clamped between 3:4 (portrait) and
/// 2:1 (panorama). Several items use a fixed 16:9 box split like Twitter does.
/// The grid is never taller than `max_height` (taller media are cropped,
/// cover fit); without one, its height follows the width. Items beyond four
/// are not shown.
pub fn layout(
    count: usize,
    first_aspect: Option<AspectRatio>,
    width: f32,
    max_height: Option<MaxHeight>,
) -> Grid {
    let width = width.max(0.0);
    let cap = max_height.map_or(f32::INFINITY, MaxHeight::px);
    let half = ((width - GAP) / 2.0).max(0.0);
    match count {
        0 => Grid { tiles: Vec::new(), height: 0.0 },
        1 => {
            let ratio = first_aspect.map_or(16.0 / 9.0, AspectRatio::as_f32).clamp(0.75, 2.0);
            let h = (width / ratio).min(cap);
            Grid { tiles: vec![Tile { x: 0.0, y: 0.0, w: width, h }], height: h }
        }
        _ => {
            let h = (width * 9.0 / 16.0).min(cap);
            let half_h = ((h - GAP) / 2.0).max(0.0);
            let right = half + GAP;
            let tiles = match count {
                2 => {
                    vec![Tile { x: 0.0, y: 0.0, w: half, h }, Tile { x: right, y: 0.0, w: half, h }]
                }
                3 => vec![
                    Tile { x: 0.0, y: 0.0, w: half, h },
                    Tile { x: right, y: 0.0, w: half, h: half_h },
                    Tile { x: right, y: half_h + GAP, w: half, h: half_h },
                ],
                _ => vec![
                    Tile { x: 0.0, y: 0.0, w: half, h: half_h },
                    Tile { x: right, y: 0.0, w: half, h: half_h },
                    Tile { x: 0.0, y: half_h + GAP, w: half, h: half_h },
                    Tile { x: right, y: half_h + GAP, w: half, h: half_h },
                ],
            };
            Grid { tiles, height: h }
        }
    }
}

/// One whole side of an image, in [`Crop`] units.
pub const CROP_UNIT: u16 = 10_000;

/// The part of an image to draw in a box, in ten-thousandths of the image's
/// width and height (whole numbers, so it can key an image cache).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Crop {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Crop {
    /// The crop in a `size` (width, height) bitmap: whole pixels, at least
    /// one per side, inside the bitmap. `None` for an empty bitmap.
    pub fn in_pixels(self, (width, height): (u32, u32)) -> Option<(u32, u32, u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let side = |at: u16, len: u16, full: u32| {
            let (unit, full) = (f64::from(CROP_UNIT), f64::from(full));
            let len = (f64::from(len) / unit * full).round().clamp(1.0, full);
            let at = (f64::from(at) / unit * full).round().clamp(0.0, full - len);
            // Both are within 0..=full, a u32.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0..=full")]
            (at as u32, len as u32)
        };
        let (x, w) = side(self.x, self.w, width);
        let (y, h) = side(self.y, self.h, height);
        Some((x, y, w, h))
    }
}

/// Where to crop `image` to fill a `box_w` × `box_h` box, like Twitter does:
/// around the suggested crop ([`ImageRef::crops`]) whose shape is closest to
/// the box's, grown to the box's shape and kept inside the image.
///
/// The crop keys the image cache, and a new key means fetching and decoding
/// the image again. So the box's shape is rounded to the size buckets' step
/// ([`image_scale::STEP`](super::image_scale::STEP)), or to the suggested
/// crop's own shape when it is within half a step: resizing a column makes a
/// new crop about as often as a new size bucket. `ObjectFit::Cover` trims
/// what the rounding leaves.
///
/// `None` when the image has no usable suggested crop or no size, or the box
/// is empty: the image is then cropped around its centre (`ObjectFit::Cover`).
pub fn crop_for(image: &ImageRef, box_w: f32, box_h: f32) -> Option<Crop> {
    let (width, height) = image.size?;
    let ratio = f64::from(box_w) / f64::from(box_h);
    if !(ratio.is_finite() && ratio > 0.0) {
        return None;
    }
    let step = f64::from(super::image_scale::STEP).ln();
    let off = |w: u32, h: u32| (f64::from(w) / f64::from(h) / ratio).ln().abs();
    let best = image
        .crops
        .iter()
        .filter(|r| r.fits_in((width, height)))
        .min_by(|a, b| off(a.w, a.h).total_cmp(&off(b.w, b.h)))?;
    let (full_w, full_h) = (f64::from(width), f64::from(height));
    let (rw, rh) = (f64::from(best.w), f64::from(best.h));
    // A suggested crop already within half a step of the box's shape is used
    // as is (Twitter's own tile shapes); otherwise the box's shape is rounded.
    let ratio = if off(best.w, best.h) <= step / 2.0 {
        rw / rh
    } else {
        ((ratio.ln() / step).round() * step).exp()
    };
    // Grow the region to the box's shape, then shrink it back into the image.
    let (w, h) = if rw / rh < ratio { (rh * ratio, rh) } else { (rw, rw / ratio) };
    let fit = (full_w / w).min(full_h / h).min(1.0);
    let (w, h) = (w * fit, h * fit);
    // Not `clamp`: rounding can leave `full - w` a hair below zero, and
    // `clamp` panics when its bounds cross.
    let x = (f64::from(best.x) + rw / 2.0 - w / 2.0).min(full_w - w).max(0.0);
    let y = (f64::from(best.y) + rh / 2.0 - h / 2.0).min(full_h - h).max(0.0);
    let unit = f64::from(CROP_UNIT);
    // Every value is a fraction of a side (0..=1) times CROP_UNIT.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0..=CROP_UNIT")]
    let units = |v: f64, full: f64| (v / full * unit).round().clamp(0.0, unit) as u16;
    let (cw, ch) = (units(w, full_w).max(1), units(h, full_h).max(1));
    Some(Crop {
        x: units(x, full_w).min(CROP_UNIT - cw),
        y: units(y, full_h).min(CROP_UNIT - ch),
        w: cw,
        h: ch,
    })
}

/// Fixed sizes of the image viewer's chrome, in pixels.
pub mod viewer {
    /// Padding around the whole viewer.
    pub const PADDING: f32 = 24.0;
    /// Gap between the header, the image row and the footer, and between the
    /// image and the ‹ › buttons.
    pub const GAP: f32 = 12.0;
    /// Height of the `n / count · @handle` header.
    pub const HEADER: f32 = 32.0;
    /// Height of the footer (alt text, clamped to two lines, and Play).
    pub const FOOTER: f32 = 48.0;
    /// Width of each ‹ › button.
    pub const NAV: f32 = 44.0;
}

/// The largest box the image viewer may draw an image in, for a window of
/// `width` × `height`. It leaves room for the chrome so the image is never
/// pushed off screen, whatever the alt text or the image size.
pub fn viewer_box(width: f32, height: f32, has_nav: bool) -> (f32, f32) {
    let nav = if has_nav { 2.0 * (viewer::NAV + viewer::GAP) } else { 0.0 };
    let w = width - 2.0 * viewer::PADDING - nav;
    let h = height - 2.0 * viewer::PADDING - viewer::HEADER - viewer::FOOTER - 2.0 * viewer::GAP;
    (w.max(0.0), h.max(0.0))
}

/// The size the image viewer draws a `bitmap` (width, height) at inside
/// `bounds` (from [`viewer_box`]): shrunk to fit with its aspect ratio kept,
/// never enlarged. The image element must be exactly this size, or clicks
/// on the letterbox around the picture land on the image instead of the
/// backdrop. `None` for an empty bitmap.
pub fn viewer_image_size(bitmap: (f32, f32), bounds: (f32, f32)) -> Option<(f32, f32)> {
    let (w, h) = bitmap;
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = (bounds.0 / w).min(bounds.1 / h).clamp(0.0, 1.0);
    Some((w * scale, h * scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use twitty_model::Region;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn viewer_image_fits_its_box_keeping_its_aspect() {
        let fits = |bitmap, bounds| viewer_image_size(bitmap, bounds).unwrap();
        // Wider than the box: the width limits, the height follows.
        let (w, h) = fits((1200.0, 800.0), (1000.0, 700.0));
        assert!(approx(w, 1000.0) && approx(h, 666.67), "{w}×{h}");
        // Taller than the box: the height limits.
        let (w, h) = fits((800.0, 1200.0), (1000.0, 600.0));
        assert!(approx(w, 400.0) && approx(h, 600.0), "{w}×{h}");
        // Smaller than the box: never blown up.
        assert_eq!(fits((300.0, 200.0), (1000.0, 700.0)), (300.0, 200.0));
        assert_eq!(fits((300.0, 200.0), (0.0, 0.0)), (0.0, 0.0));
        assert_eq!(viewer_image_size((0.0, 200.0), (1000.0, 700.0)), None);
    }

    #[test]
    fn single_keeps_aspect() {
        let g = layout(1, AspectRatio::new(4, 3), 300.0, None);
        assert!(approx(g.height, 225.0));
    }

    #[test]
    fn single_clamps_tall_and_wide() {
        assert!(approx(layout(1, AspectRatio::new(1, 10), 210.0, None).height, 280.0));
        assert!(approx(layout(1, AspectRatio::new(10, 1), 300.0, None).height, 150.0));
    }

    #[test]
    fn grid_height_is_not_capped() {
        // A 3:4 portrait in a 300px column is shown 400px high, uncropped.
        let g = layout(1, AspectRatio::new(3, 4), 300.0, None);
        assert!(approx(g.height, 400.0) && approx(g.tiles[0].h, 400.0));
        assert!(approx(g.tiles[0].w, 300.0));
        // Several items in a wide column keep their 16:9 box.
        for n in 2..=4 {
            let g = layout(n, None, 800.0, None);
            assert!(approx(g.height, 450.0), "n={n}");
            assert!(g.tiles.iter().all(|t| t.y + t.h <= 450.0 + 0.01), "n={n}");
        }
    }

    #[test]
    fn grid_is_never_taller_than_the_cap() {
        let cap = Some(MaxHeight::new(300.0));
        // A 3:4 portrait in a 300px column would be 400px high.
        let g = layout(1, AspectRatio::new(3, 4), 300.0, cap);
        assert!(approx(g.height, 300.0) && approx(g.tiles[0].h, 300.0));
        assert!(approx(g.tiles[0].w, 300.0));
        // Several items in a wide column: the 16:9 box is capped too.
        for n in 2..=4 {
            let g = layout(n, None, 800.0, cap);
            assert!(approx(g.height, 300.0), "n={n}");
            assert!(g.tiles.iter().all(|t| t.y + t.h <= 300.0 + 0.01), "n={n}");
        }
        // Shorter media are not stretched to the cap.
        assert!(approx(layout(1, None, 320.0, cap).height, 180.0));
    }

    #[test]
    fn max_height_stays_in_range() {
        assert_eq!(MaxHeight::new(10.0).px(), f32::from(MaxHeight::MIN));
        assert_eq!(MaxHeight::new(5000.0).px(), f32::from(MaxHeight::MAX));
        assert_eq!(MaxHeight::new(f32::NAN).px(), f32::from(MaxHeight::MIN));
        assert_eq!(MaxHeight::new(299.6).px(), 300.0);
        assert_eq!(MaxHeight::from(1u16), MaxHeight::new(0.0), "a hand-edited file is clamped");
    }

    #[test]
    fn the_sliders_last_step_means_no_limit() {
        let end = MaxHeight::slider_value(None);
        assert_eq!(end, f32::from(MaxHeight::MAX + MaxHeight::STEP));
        assert_eq!(MaxHeight::from_slider(end), None);
        assert_eq!(MaxHeight::from_slider(f32::from(MaxHeight::MAX)), Some(MaxHeight::new(800.0)));
        for px in [150.0, 300.0, 800.0] {
            let limit = Some(MaxHeight::new(px));
            assert_eq!(MaxHeight::from_slider(MaxHeight::slider_value(limit)), limit);
        }
        assert_eq!(MaxHeight::label(None), "No limit");
        assert_eq!(MaxHeight::label(Some(MaxHeight::new(300.0))), "300 px");
    }

    #[test]
    fn single_without_aspect_is_16_9() {
        assert!(approx(layout(1, None, 320.0, None).height, 180.0));
    }

    #[test]
    fn tile_counts_cap_at_four() {
        for (n, tiles) in [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (7, 4)] {
            assert_eq!(layout(n, None, 300.0, None).tiles.len(), tiles, "count {n}");
        }
    }

    #[test]
    fn tiles_stay_inside_grid_and_do_not_overlap() {
        for n in 1..=4 {
            let g = layout(n, AspectRatio::new(3, 2), 288.0, None);
            for (i, a) in g.tiles.iter().enumerate() {
                assert!(a.x >= 0.0 && a.y >= 0.0, "n={n}");
                assert!(a.x + a.w <= 288.0 + 0.01 && a.y + a.h <= g.height + 0.01, "n={n}");
                for b in &g.tiles[i + 1..] {
                    let overlap = a.x < b.x + b.w - 0.01
                        && b.x < a.x + a.w - 0.01
                        && a.y < b.y + b.h - 0.01
                        && b.y < a.y + a.h - 0.01;
                    assert!(!overlap, "n={n}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn zero_width_does_not_go_negative() {
        let g = layout(4, None, 0.0, None);
        assert!(g.tiles.iter().all(|t| t.w >= 0.0 && t.h >= 0.0));
    }

    #[test]
    fn only_outer_corners_are_rounded() {
        let w = 300.0;
        let c = |n: usize| {
            let g = layout(n, None, w, None);
            g.tiles.iter().map(|t| g.corners(t, w)).collect::<Vec<_>>()
        };
        assert_eq!(c(1), vec![Corners::ALL]);

        let two = c(2);
        assert_eq!(two[0], Corners { top_left: true, bottom_left: true, ..Corners::default() });
        assert_eq!(two[1], Corners { top_right: true, bottom_right: true, ..Corners::default() });

        let three = c(3);
        assert_eq!(three[0], Corners { top_left: true, bottom_left: true, ..Corners::default() });
        assert_eq!(three[1], Corners { top_right: true, ..Corners::default() });
        assert_eq!(three[2], Corners { bottom_right: true, ..Corners::default() });

        let four = c(4);
        assert_eq!(four[0], Corners { top_left: true, ..Corners::default() });
        assert_eq!(four[1], Corners { top_right: true, ..Corners::default() });
        assert_eq!(four[2], Corners { bottom_left: true, ..Corners::default() });
        assert_eq!(four[3], Corners { bottom_right: true, ..Corners::default() });
    }

    #[test]
    fn every_grid_corner_is_covered_exactly_once() {
        let w = 288.0;
        for n in 1..=4 {
            let g = layout(n, AspectRatio::new(3, 2), w, None);
            let cs: Vec<Corners> = g.tiles.iter().map(|t| g.corners(t, w)).collect();
            let count = |f: fn(&Corners) -> bool| cs.iter().filter(|c| f(c)).count();
            assert_eq!(count(|c| c.top_left), 1, "n={n}");
            assert_eq!(count(|c| c.top_right), 1, "n={n}");
            assert_eq!(count(|c| c.bottom_left), 1, "n={n}");
            assert_eq!(count(|c| c.bottom_right), 1, "n={n}");
        }
    }
    #[test]
    fn viewer_box_leaves_room_for_the_chrome() {
        let (w, h) = viewer_box(1280.0, 800.0, false);
        assert!(approx(w, 1280.0 - 48.0));
        assert!(approx(h, 800.0 - 48.0 - 32.0 - 48.0 - 24.0));
        let (w_nav, h_nav) = viewer_box(1280.0, 800.0, true);
        assert!(approx(w_nav, w - 2.0 * (44.0 + 12.0)));
        assert!(approx(h_nav, h));
    }

    #[test]
    fn viewer_box_fits_in_the_window_and_never_goes_negative() {
        for (ww, wh) in [(0.0, 0.0), (50.0, 50.0), (640.0, 360.0), (3840.0, 2160.0)] {
            for nav in [false, true] {
                let (w, h) = viewer_box(ww, wh, nav);
                assert!(w >= 0.0 && h >= 0.0, "{ww}x{wh}");
                assert!(w <= ww && h <= wh, "{ww}x{wh}");
            }
        }
    }

    fn region(x: u32, y: u32, w: u32, h: u32) -> Region {
        Region { x, y, w, h }
    }

    fn photo(size: (u32, u32), crops: &[Region]) -> ImageRef {
        let url = "https://pbs.twimg.com/media/a.jpg".parse().unwrap();
        ImageRef::sized(url, Some(size)).with_crops(crops.iter().copied())
    }

    /// A 1075×1217 photo with the five crops Twitter gave it (16:9, 1:1,
    /// 0.88, 1:2, whole), from the `home` fixture.
    fn portrait() -> ImageRef {
        photo(
            (1075, 1217),
            &[
                region(0, 0, 1075, 602),
                region(0, 0, 1075, 1075),
                region(0, 0, 1068, 1217),
                region(151, 0, 609, 1217),
                region(0, 0, 1075, 1217),
            ],
        )
    }

    /// `crop` in pixels of a `size` image.
    fn pixels(crop: Option<Crop>, size: (u32, u32)) -> (u32, u32, u32, u32) {
        crop.unwrap().in_pixels(size).unwrap()
    }

    #[test]
    fn a_tile_shows_the_crop_of_its_shape_exactly() {
        let p = portrait();
        let px = |w, h| pixels(crop_for(&p, w, h), (1075, 1217));
        // A 16:9 tile shows the top of the portrait, not its middle.
        assert_eq!(px(320.0, 180.0), (0, 0, 1075, 602));
        // A 1:2 tile shows the suggested off-centre strip.
        assert_eq!(px(100.0, 200.0), (151, 0, 609, 1217));
        // In its own shape, the photo is shown whole.
        assert_eq!(px(1075.0, 1217.0), (0, 0, 1075, 1217));
    }

    #[test]
    fn a_box_between_two_shapes_grows_the_closest_crop() {
        // 3:2 is closest to the 16:9 strip at the top: grown downwards from
        // it, not from the middle of the photo.
        let (x, y, w, h) = pixels(crop_for(&portrait(), 300.0, 200.0), (1075, 1217));
        assert_eq!((x, y, w), (0, 0, 1075));
        assert!((602..1075).contains(&h), "{h}");
    }

    #[test]
    fn a_grown_crop_is_moved_back_into_the_image() {
        // A square at the right edge of a 3:1 photo, in a 3:1 box: grown to
        // the whole width, so pushed back left.
        let size = (300, 100);
        let p = photo(size, &[region(200, 0, 100, 100)]);
        let (x, _, w, _) = pixels(crop_for(&p, 3.0, 1.0), size);
        assert_eq!((x, w), (0, 300));
        // In a 1:2 box: shrunk into the image, still centred on the square.
        let (x, y, w, h) = pixels(crop_for(&p, 1.0, 2.0), size);
        assert_eq!((y, h), (0, 100));
        assert!((x + w / 2).abs_diff(250) <= 1 && w < 100, "{x} {w}");
    }

    #[test]
    fn without_a_usable_crop_or_size_the_centre_is_shown() {
        assert_eq!(crop_for(&photo((100, 100), &[]), 16.0, 9.0), None);
        let mut no_size = portrait();
        no_size.size = None;
        assert_eq!(crop_for(&no_size, 16.0, 9.0), None);
        // Pushed in by hand, past `with_crops`' check.
        let mut outside = photo((100, 100), &[]);
        outside.crops = vec![region(50, 0, 100, 100)];
        assert_eq!(crop_for(&outside, 16.0, 9.0), None);
        for (w, h) in [(0.0, 9.0), (16.0, 0.0), (f32::NAN, 9.0), (f32::INFINITY, 9.0)] {
            assert_eq!(crop_for(&portrait(), w, h), None, "{w}×{h}");
        }
    }

    #[test]
    fn resizing_a_column_recrops_no_more_often_than_it_resizes() {
        // A single portrait capped at 300px: its tile's shape follows the
        // column's width (1:1 to 3:1 here). Each new crop reloads the image,
        // like each new size bucket does. The shape steps match the buckets;
        // snapping to the 16:9 crop on the way splits one step in three, so
        // at most two more.
        let p = portrait();
        let cap = Some(MaxHeight::new(300.0));
        let (mut crops, mut buckets) = (Vec::new(), Vec::new());
        for w in 300..=900 {
            let tile = layout(1, p.aspect(), w as f32, cap).tiles[0];
            let (w, h) = (tile.w, tile.h);
            let crop = crop_for(&p, w, h);
            let bucket = crate::state::image_scale::BoxPx::bucket(w, h, 1.0);
            if crops.last() != Some(&crop) {
                crops.push(crop);
            }
            if buckets.last() != Some(&bucket) {
                buckets.push(bucket);
            }
        }
        let (crops, buckets) = (crops.len(), buckets.len());
        assert!(crops <= buckets + 2, "{crops} crops, {buckets} buckets");
    }

    #[test]
    fn a_crop_in_pixels_stays_in_the_bitmap() {
        let c = Crop { x: 9_999, y: 0, w: 1, h: CROP_UNIT };
        assert_eq!(c.in_pixels((10, 10)), Some((9, 0, 1, 10)));
        assert_eq!(c.in_pixels((0, 10)), None);
    }

    /// Fractions of an image (x, y, w, h), each side at least 5 %.
    fn fractions() -> impl Strategy<Value = (f64, f64, f64, f64)> {
        (0.0f64..1.0, 0.0f64..1.0, 0.05f64..=1.0, 0.05f64..=1.0)
    }

    proptest! {
        #[test]
        fn a_crop_is_inside_the_image_and_has_the_boxs_shape(
            width in 50u32..5000,
            height in 50u32..5000,
            rects in proptest::collection::vec(fractions(), 1..6),
            box_w in 20.0f32..1000.0,
            box_h in 20.0f32..1000.0,
        ) {
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0..=side")]
            let at = |f: f64, side: u32| (f * f64::from(side)) as u32;
            let crops: Vec<Region> = rects
                .iter()
                .map(|&(x, y, w, h)| {
                    let (w, h) = (at(w, width).max(1), at(h, height).max(1));
                    region(at(x, width).min(width - w), at(y, height).min(height - h), w, h)
                })
                .collect();
            let c = crop_for(&photo((width, height), &crops), box_w, box_h).unwrap();
            prop_assert!(c.w > 0 && c.h > 0);
            prop_assert!(c.x + c.w <= CROP_UNIT && c.y + c.h <= CROP_UNIT, "{c:?}");
            let (x, y, w, h) = c.in_pixels((width, height)).unwrap();
            prop_assert!(x + w <= width && y + h <= height);
            // The box's shape, give or take half a ratio step, then rounding:
            // a pixel plus a crop unit per side.
            let slack = f64::from(super::super::image_scale::STEP).ln() / 2.0
                + (1.0 + f64::from(width) / f64::from(CROP_UNIT)) / f64::from(w)
                + (1.0 + f64::from(height) / f64::from(CROP_UNIT)) / f64::from(h);
            let skew = (f64::from(w) / f64::from(h) / (f64::from(box_w) / f64::from(box_h))).ln();
            prop_assert!(skew.abs() <= slack * 1.01, "{w}×{h} in {box_w}×{box_h}");
        }
    }
}
