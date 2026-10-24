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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
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
}
