//! Timing for the UI's animations: which rows just arrived, which toggles
//! just turned on, and the curves that turn an animation's progress into
//! offsets and opacities. The views only feed these into `with_animation`.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

use super::column::{ListChange, LoadKind};

/// How long a row that just arrived takes to fade in.
pub const ARRIVAL: Duration = Duration::from_millis(260);
/// How long the tint of a row that arrived at the top (a newer tweet) lasts.
pub const ARRIVAL_HIGHLIGHT: Duration = Duration::from_millis(1400);
/// Delay between the rows of a freshly loaded page, so they cascade in.
pub const STAGGER: Duration = Duration::from_millis(35);
/// Rows past this many on a fresh page appear with the last one (they are
/// usually off screen anyway).
pub const STAGGER_ROWS: usize = 8;
/// How far (px) an arriving row, a dialog card or the "new tweets" pill travels.
pub const TRAVEL: f32 = 8.0;
/// How far (px) the thread panel slides in from the right edge. Short, so the
/// panel's hitbox is already where the user expects it on the first frame.
pub const PANEL_TRAVEL: f32 = 48.0;
/// The like / retweet / bookmark "pop".
pub const POP: Duration = Duration::from_millis(320);
/// How much bigger an icon gets at the top of its pop.
pub const POP_SCALE: f32 = 0.35;

/// How a row enters the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrance {
    /// Part of a freshly loaded page: fades in after `delay`.
    Cascade { delay: Duration },
    /// A newer tweet at the top: drops in and is tinted for a moment.
    Newer,
    /// An older row appended at the bottom: fades in.
    Older,
}

impl Entrance {
    /// The entrances for a freshly loaded page of `count` rows.
    pub fn cascade(count: usize) -> impl Iterator<Item = Entrance> {
        (0..count).map(|ix| Entrance::Cascade {
            delay: STAGGER * u32::try_from(ix.min(STAGGER_ROWS)).unwrap_or(u32::MAX),
        })
    }

    /// The whole animation, delay included.
    pub fn total(self) -> Duration {
        match self {
            Entrance::Cascade { delay } => delay + ARRIVAL,
            Entrance::Newer => ARRIVAL_HIGHLIGHT,
            Entrance::Older => ARRIVAL,
        }
    }

    /// The row's look at `t` (0..=1 over [`Self::total`]).
    pub fn frame(self, t: f32) -> RowFrame {
        let total = self.total().as_secs_f32();
        let delay = match self {
            Entrance::Cascade { delay } => delay.as_secs_f32(),
            _ => 0.0,
        };
        let appear =
            ease_out_cubic(segment(t, delay / total, (delay + ARRIVAL.as_secs_f32()) / total));
        let (offset, tint) = match self {
            Entrance::Newer => (-TRAVEL * (1.0 - appear), 1.0 - t.clamp(0.0, 1.0)),
            Entrance::Cascade { .. } => (TRAVEL * (1.0 - appear), 0.0),
            Entrance::Older => (0.0, 0.0),
        };
        RowFrame { opacity: appear, offset, tint }
    }
}

/// The rows (by index after the change) that enter with `change` from a
/// load of `kind`, and how. Only the first page of a reset cascades: further
/// rows are far off screen. Newer rows drop in wherever they go (below the
/// pinned tweets, if any).
pub fn entrances(kind: LoadKind, change: &ListChange) -> Vec<(usize, Entrance)> {
    match change {
        ListChange::None => Vec::new(),
        ListChange::Reset { count } => {
            Entrance::cascade((*count).min(STAGGER_ROWS * 2)).enumerate().collect()
        }
        ListChange::Splice { range, count } => {
            let entrance = if kind == LoadKind::Newer { Entrance::Newer } else { Entrance::Older };
            (range.start..range.start + count).map(|ix| (ix, entrance)).collect()
        }
    }
}

/// One frame of a row's entrance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowFrame {
    pub opacity: f32,
    /// Vertical offset in px (layout is unchanged; only the paint moves).
    pub offset: f32,
    /// Strength (0..=1) of the "new" tint behind the row.
    pub tint: f32,
}

/// Size factor of an icon `t` (0..=1) through its pop: 1 → 1 + POP_SCALE → 1.
pub fn pop_scale(t: f32) -> f32 {
    // `sin(PI)` is a hair below zero in f32: never shrink past rest.
    1.0 + POP_SCALE * (std::f32::consts::PI * t.clamp(0.0, 1.0)).sin().max(0.0)
}

/// Progress through the part of `t` between `start` and `end`, clamped to 0..=1.
pub fn segment(t: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return if t >= end { 1.0 } else { 0.0 };
    }
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Things (rows, toggles) whose animation started recently, each with a value
/// describing it. Entries are forgotten once their animation is over, so a row
/// that scrolls back into view later doesn't animate again.
#[derive(Debug, Clone)]
pub struct Recent<K, V> {
    entries: HashMap<K, (Instant, V)>,
}

impl<K: Eq + Hash, V> Default for Recent<K, V> {
    fn default() -> Self {
        Self { entries: HashMap::new() }
    }
}

impl<K: Eq + Hash, V> Recent<K, V> {
    /// Remembers `key` until `now + lasts`; also forgets expired entries.
    pub fn mark(&mut self, key: K, value: V, now: Instant, lasts: Duration) {
        self.prune(now);
        self.entries.insert(key, (now + lasts, value));
    }

    /// The value for `key` while its animation is still running at `now`.
    pub fn get(&self, key: &K, now: Instant) -> Option<&V> {
        self.entries.get(key).filter(|(until, _)| *until > now).map(|(_, v)| v)
    }

    /// Forgets entries whose animation is over at `now`.
    pub fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, (until, _)| *until > now);
    }

    pub fn remove(&mut self, key: &K) {
        self.entries.remove(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn recent_entries_expire() {
        let t0 = Instant::now();
        let mut r = Recent::default();
        r.mark("a", 1, t0, Duration::from_millis(100));
        assert_eq!(r.get(&"a", t0), Some(&1));
        assert_eq!(r.get(&"a", t0 + Duration::from_millis(99)), Some(&1));
        assert_eq!(r.get(&"a", t0 + Duration::from_millis(100)), None);
        assert_eq!(r.get(&"b", t0), None);
    }

    #[test]
    fn marking_forgets_expired_entries() {
        let t0 = Instant::now();
        let mut r = Recent::default();
        r.mark("a", (), t0, Duration::from_millis(10));
        r.mark("b", (), t0 + Duration::from_millis(50), Duration::from_millis(10));
        assert_eq!(r.len(), 1, "a was over when b arrived");
        r.mark("b", (), t0 + Duration::from_millis(55), Duration::from_millis(10));
        assert_eq!(r.len(), 1, "re-marking replaces");
    }

    #[test]
    fn cascade_staggers_the_first_rows_only() {
        let delays: Vec<_> = Entrance::cascade(STAGGER_ROWS + 3)
            .map(|e| match e {
                Entrance::Cascade { delay } => delay,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(delays[0], Duration::ZERO);
        assert_eq!(delays[1], STAGGER);
        assert!(delays.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(delays[STAGGER_ROWS], delays[STAGGER_ROWS + 2], "capped");
    }

    #[test]
    fn entrances_start_hidden_and_end_at_rest() {
        let entrances =
            [Entrance::Newer, Entrance::Older, Entrance::Cascade { delay: STAGGER * 4 }];
        for e in entrances {
            let start = e.frame(0.0);
            assert_eq!(start.opacity, 0.0, "{e:?}");
            assert_eq!(e.frame(1.0), RowFrame { opacity: 1.0, offset: 0.0, tint: 0.0 }, "{e:?}");
        }
        assert!(Entrance::Newer.frame(0.0).offset < 0.0, "newer tweets drop in from above");
        assert_eq!(Entrance::Newer.frame(0.0).tint, 1.0);
        // A delayed row is still hidden during its delay.
        let late = Entrance::Cascade { delay: STAGGER * 4 };
        let during_delay = (STAGGER * 3).as_secs_f32() / late.total().as_secs_f32();
        assert_eq!(late.frame(during_delay).opacity, 0.0);
    }

    #[test]
    fn list_changes_pick_their_entrances() {
        use LoadKind::{Newer, Older, Refresh};
        assert!(entrances(Newer, &ListChange::None).is_empty());
        let newer = entrances(Newer, &ListChange::Splice { range: 0..0, count: 2 });
        assert_eq!(newer, [(0, Entrance::Newer), (1, Entrance::Newer)]);
        let below_pin = entrances(Newer, &ListChange::Splice { range: 1..1, count: 1 });
        assert_eq!(below_pin, [(1, Entrance::Newer)]);
        let older = entrances(Older, &ListChange::Splice { range: 20..20, count: 2 });
        assert_eq!(older, [(20, Entrance::Older), (21, Entrance::Older)]);
        let reset = entrances(Refresh, &ListChange::Reset { count: 100 });
        assert_eq!(reset.len(), STAGGER_ROWS * 2, "only the first screenful");
        assert_eq!(reset[0], (0, Entrance::Cascade { delay: Duration::ZERO }));
        assert!(entrances(Refresh, &ListChange::Reset { count: 0 }).is_empty());
    }

    #[test]
    fn pop_returns_to_rest() {
        assert_eq!(pop_scale(0.0), 1.0);
        assert!((pop_scale(0.5) - (1.0 + POP_SCALE)).abs() < 1e-6);
        assert!((pop_scale(1.0) - 1.0).abs() < 1e-6);
    }

    proptest! {
        #[test]
        fn frames_stay_in_range(t in -1.0f32..2.0, delay_ms in 0u64..1000) {
            let entrances = [
                Entrance::Newer,
                Entrance::Older,
                Entrance::Cascade { delay: Duration::from_millis(delay_ms) },
            ];
            for e in entrances {
                let f = e.frame(t);
                prop_assert!((0.0..=1.0).contains(&f.opacity));
                prop_assert!((0.0..=1.0).contains(&f.tint));
                prop_assert!(f.offset.abs() <= TRAVEL);
            }
            prop_assert!((1.0..=1.0 + POP_SCALE + 1e-6).contains(&pop_scale(t)));
        }

        #[test]
        fn segment_is_clamped(t in -10.0f32..10.0, a in -1.0f32..1.0, b in -1.0f32..1.0) {
            prop_assert!((0.0..=1.0).contains(&segment(t, a, b)));
        }
    }
}
