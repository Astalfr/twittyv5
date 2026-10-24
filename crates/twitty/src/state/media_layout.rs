//! Geometry of the 1–4 item media grid, in the style of X.

use twitty_model::AspectRatio;

/// Gap between tiles, in pixels.
pub const GAP: f32 = 2.0;

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
/// 2:1 (panorama). Several items use a fixed 16:9 box split like X does.
/// Items beyond four are not shown.
pub fn layout(count: usize, first_aspect: Option<AspectRatio>, width: f32) -> Grid {
    let width = width.max(0.0);
    let half = ((width - GAP) / 2.0).max(0.0);
    match count {
        0 => Grid { tiles: Vec::new(), height: 0.0 },
        1 => {
            let ratio = first_aspect.map_or(16.0 / 9.0, AspectRatio::as_f32).clamp(0.75, 2.0);
            let h = width / ratio;
            Grid { tiles: vec![Tile { x: 0.0, y: 0.0, w: width, h }], height: h }
        }
        _ => {
            let h = width * 9.0 / 16.0;
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
        let g = layout(1, AspectRatio::new(4, 3), 300.0);
        assert!(approx(g.height, 225.0));
    }

    #[test]
    fn single_clamps_tall_and_wide() {
        assert!(approx(layout(1, AspectRatio::new(1, 10), 300.0).height, 400.0));
        assert!(approx(layout(1, AspectRatio::new(10, 1), 300.0).height, 150.0));
    }

    #[test]
    fn single_without_aspect_is_16_9() {
        assert!(approx(layout(1, None, 320.0).height, 180.0));
    }

    #[test]
    fn tile_counts_cap_at_four() {
        for (n, tiles) in [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (7, 4)] {
            assert_eq!(layout(n, None, 300.0).tiles.len(), tiles, "count {n}");
        }
    }

    #[test]
    fn tiles_stay_inside_grid_and_do_not_overlap() {
        for n in 1..=4 {
            let g = layout(n, AspectRatio::new(3, 2), 288.0);
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
        let g = layout(4, None, 0.0);
        assert!(g.tiles.iter().all(|t| t.w >= 0.0 && t.h >= 0.0));
    }

    #[test]
    fn only_outer_corners_are_rounded() {
        let w = 300.0;
        let c = |n: usize| {
            let g = layout(n, None, w);
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
            let g = layout(n, AspectRatio::new(3, 2), w);
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
}
