//! Middle-click autoscroll, like Chrome's: a middle press drops an anchor, and
//! while it lasts the view under the anchor scrolls towards the pointer, faster
//! the farther the pointer is from the anchor.
//!
//! - **Hold**: press, move (or hold the button a moment), release. Scrolling
//!   stops on release.
//! - **Toggle**: a quick middle click. Scrolling goes on until the next press
//!   of any button (which does nothing else), Esc, or a wheel turn.
//!
//! Positions are window coordinates in logical pixels, y pointing down.

use std::time::{Duration, Instant};

/// Distance (px, per axis) around the anchor where nothing scrolls. Moving the
/// pointer farther than this while the button is down makes the press a hold.
pub const DEAD_ZONE: f32 = 15.0;
/// A press held at least this long is a hold, even if the pointer didn't move.
pub const HOLD: Duration = Duration::from_millis(300);
/// Speed (px/s) per pixel past the dead zone.
const LINEAR: f32 = 4.0;
/// Extra speed (px/s) per squared pixel past the dead zone, so far moves race.
const QUADRATIC: f32 = 0.04;
/// Top speed (px/s) on each axis.
pub const MAX_SPEED: f32 = 8000.0;

/// A point or a distance in window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The middle button is down and the pointer hasn't left the dead zone.
    Pressed,
    /// The middle button is down and the pointer left the dead zone: the
    /// release stops scrolling.
    Holding,
    /// Released after a quick click: scrolls until the next press.
    Latched,
}

/// What the middle button's release does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Keep scrolling (it was a quick click).
    Keep,
    /// Stop (it was a hold).
    Stop,
}

/// One autoscroll, from the middle press that starts it.
#[derive(Debug, Clone, PartialEq)]
pub struct Autoscroll {
    anchor: Vec2,
    pointer: Vec2,
    pressed_at: Instant,
    mode: Mode,
}

impl Autoscroll {
    /// A middle press at `at`.
    pub fn start(at: Vec2, now: Instant) -> Self {
        Self { anchor: at, pointer: at, pressed_at: now, mode: Mode::Pressed }
    }

    /// Where the scrolling view is picked: the press's position.
    pub fn anchor(&self) -> Vec2 {
        self.anchor
    }

    /// Where the pointer last was.
    pub fn pointer(&self) -> Vec2 {
        self.pointer
    }

    /// Whether the middle button is still down.
    pub fn is_held(&self) -> bool {
        self.mode != Mode::Latched
    }

    /// The pointer moved to `at`.
    pub fn pointer_moved(&mut self, at: Vec2) {
        self.pointer = at;
        let (dx, dy) = (at.x - self.anchor.x, at.y - self.anchor.y);
        if self.mode == Mode::Pressed && (dx.abs() > DEAD_ZONE || dy.abs() > DEAD_ZONE) {
            self.mode = Mode::Holding;
        }
    }

    /// The middle button was released.
    pub fn released(&mut self, now: Instant) -> Release {
        match self.mode {
            Mode::Pressed if now.saturating_duration_since(self.pressed_at) < HOLD => {
                self.mode = Mode::Latched;
                Release::Keep
            }
            Mode::Latched => Release::Keep,
            Mode::Pressed | Mode::Holding => Release::Stop,
        }
    }

    /// Scroll speed (px/s) towards the pointer: positive x is rightwards,
    /// positive y downwards (towards later content).
    pub fn velocity(&self) -> Vec2 {
        Vec2::new(
            axis_speed(self.pointer.x - self.anchor.x),
            axis_speed(self.pointer.y - self.anchor.y),
        )
    }

    /// How far (px) to scroll after `elapsed`, in [`Self::velocity`]'s
    /// directions.
    pub fn step(&self, elapsed: Duration) -> Vec2 {
        let v = self.velocity();
        let s = elapsed.as_secs_f32();
        Vec2::new(v.x * s, v.y * s)
    }
}

/// Speed on one axis for a pointer `offset` px from the anchor.
fn axis_speed(offset: f32) -> f32 {
    let past = (offset.abs() - DEAD_ZONE).max(0.0);
    let speed = (LINEAR * past + QUADRATIC * past * past).min(MAX_SPEED);
    speed.copysign(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> (Autoscroll, Instant) {
        let now = Instant::now();
        (Autoscroll::start(Vec2::new(100.0, 100.0), now), now)
    }

    #[test]
    fn nothing_scrolls_inside_the_dead_zone() {
        let (mut a, _) = started();
        assert_eq!(a.velocity(), Vec2::default());
        a.pointer_moved(Vec2::new(100.0 + DEAD_ZONE, 100.0 - DEAD_ZONE));
        assert_eq!(a.velocity(), Vec2::default());
    }

    #[test]
    fn scrolls_towards_the_pointer_faster_farther_away() {
        let (mut a, _) = started();
        a.pointer_moved(Vec2::new(100.0, 160.0));
        let near = a.velocity().y;
        assert!(near > 0.0, "below the anchor scrolls down");
        a.pointer_moved(Vec2::new(100.0, 300.0));
        assert!(a.velocity().y > near);
        a.pointer_moved(Vec2::new(40.0, 100.0));
        let v = a.velocity();
        assert!(v.x < 0.0, "left of the anchor scrolls left");
        assert_eq!(v.y, 0.0);
    }

    #[test]
    fn speed_is_symmetric_and_capped() {
        let (mut a, _) = started();
        a.pointer_moved(Vec2::new(180.0, 20.0));
        let v = a.velocity();
        assert_eq!(v.x, -v.y);
        a.pointer_moved(Vec2::new(100.0, 100_000.0));
        assert_eq!(a.velocity().y, MAX_SPEED);
    }

    #[test]
    fn step_is_speed_times_elapsed() {
        let (mut a, _) = started();
        a.pointer_moved(Vec2::new(100.0, 100.0 + DEAD_ZONE + 10.0));
        let v = a.velocity().y;
        let step = a.step(Duration::from_millis(500)).y;
        assert!((step - v / 2.0).abs() < 1e-3);
    }

    #[test]
    fn a_quick_click_latches_until_the_next_press() {
        let (mut a, now) = started();
        a.pointer_moved(Vec2::new(105.0, 104.0)); // a jitter, inside the dead zone
        assert_eq!(a.released(now + Duration::from_millis(80)), Release::Keep);
        // Once latched, moving far and releasing again doesn't stop it.
        a.pointer_moved(Vec2::new(100.0, 400.0));
        assert_eq!(a.released(now + Duration::from_secs(2)), Release::Keep);
        assert!(a.velocity().y > 0.0);
    }

    #[test]
    fn a_drag_stops_on_release() {
        let (mut a, now) = started();
        a.pointer_moved(Vec2::new(100.0, 200.0));
        // Back near the anchor before releasing: still a hold.
        a.pointer_moved(Vec2::new(100.0, 100.0));
        assert_eq!(a.released(now + Duration::from_millis(80)), Release::Stop);
    }

    #[test]
    fn a_long_press_stops_on_release() {
        let (mut a, now) = started();
        assert_eq!(a.released(now + HOLD), Release::Stop);
    }
}
