//! The side sheets (threads, profiles) open over the deck's right side. They
//! stack: a new one goes on top, and the ones under it shift left so their
//! left edge stays visible.

/// Most sheets kept open; opening one more drops the oldest.
pub const MAX_SHEETS: usize = 8;
/// How far left each sheet under the top one shifts, in pixels.
pub const PEEK: f32 = 32.0;
/// Sheets under the top one that show an edge. Deeper ones sit behind the
/// last of them, so the stack never runs far across the deck.
pub const MAX_PEEKS: usize = 5;

/// How far left of the top sheet the sheet `depth` levels under it sits.
pub fn offset(depth: usize) -> f32 {
    depth.min(MAX_PEEKS) as f32 * PEEK
}

/// The last change to a stack, which says where its sheets come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// One sheet went on top: the others moved one level down.
    Pushed,
    /// This many sheets came off the top: the others moved up as many levels.
    Popped(usize),
}

/// The open sheets, the bottom one first.
#[derive(Debug)]
pub struct SheetStack<T> {
    sheets: Vec<T>,
    last: Change,
}

impl<T> Default for SheetStack<T> {
    fn default() -> Self {
        Self { sheets: Vec::new(), last: Change::Pushed }
    }
}

impl<T> SheetStack<T> {
    /// Puts `sheet` on top. Returns the oldest sheet when the stack was full.
    pub fn push(&mut self, sheet: T) -> Option<T> {
        let dropped = (self.sheets.len() >= MAX_SHEETS).then(|| self.sheets.remove(0));
        self.sheets.push(sheet);
        self.last = Change::Pushed;
        dropped
    }

    /// Takes the top sheet off.
    pub fn pop(&mut self) -> Option<T> {
        let top = self.sheets.pop()?;
        self.last = Change::Popped(1);
        Some(top)
    }

    /// Takes off the first sheet `matches` picks, wherever it is.
    pub fn remove(&mut self, matches: impl Fn(&T) -> bool) -> Option<T> {
        let ix = self.sheets.iter().position(matches)?;
        let was_top = ix + 1 == self.sheets.len();
        let sheet = self.sheets.remove(ix);
        if was_top {
            self.last = Change::Popped(1);
        }
        Some(sheet)
    }

    /// Takes every sheet off, the bottom one first.
    pub fn clear(&mut self) -> Vec<T> {
        let all = std::mem::take(&mut self.sheets);
        if !all.is_empty() {
            self.last = Change::Popped(all.len());
        }
        all
    }

    /// The live sheet.
    pub fn top(&self) -> Option<&T> {
        self.sheets.last()
    }

    /// Every open sheet, the bottom one first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.sheets.iter()
    }

    /// How many sheets are open.
    pub fn len(&self) -> usize {
        self.sheets.len()
    }

    /// Whether no sheet is open.
    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// The sheets to draw, each with its depth (0 is the top), the deepest
    /// first so the top one paints over the others. Sheets hidden behind the
    /// last visible edge are left out.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &T)> {
        let shown = self.sheets.len().min(MAX_PEEKS + 1);
        let top = self.sheets.len().saturating_sub(1);
        self.sheets
            .iter()
            .enumerate()
            .skip(self.sheets.len() - shown)
            .map(move |(ix, s)| (top - ix, s))
    }

    /// Where the sheet now at `depth` was before the last change, or `None`
    /// for a sheet that just went on top.
    pub fn moved_from(&self, depth: usize) -> Option<usize> {
        match self.last {
            Change::Pushed => depth.checked_sub(1),
            Change::Popped(n) => Some(depth + n),
        }
    }

    /// The last change to the stack.
    pub fn last_change(&self) -> Change {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(n: usize) -> SheetStack<usize> {
        let mut s = SheetStack::default();
        for i in 0..n {
            s.push(i);
        }
        s
    }

    fn depths(s: &SheetStack<usize>) -> Vec<(usize, usize)> {
        s.visible().map(|(d, v)| (d, *v)).collect()
    }

    #[test]
    fn a_new_sheet_goes_on_top() {
        let mut s = stack(2);
        assert_eq!(s.top(), Some(&1));
        assert_eq!(s.push(2), None);
        assert_eq!(s.top(), Some(&2));
        assert_eq!(s.len(), 3);
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(s.last_change(), Change::Pushed);
    }

    #[test]
    fn a_full_stack_drops_its_oldest_sheet() {
        let mut s = stack(MAX_SHEETS);
        assert_eq!(s.push(99), Some(0));
        assert_eq!(s.len(), MAX_SHEETS);
        assert_eq!(s.top(), Some(&99));
    }

    #[test]
    fn popping_shows_the_sheet_under_it() {
        let mut s = stack(2);
        assert_eq!(s.pop(), Some(1));
        assert_eq!(s.top(), Some(&0));
        assert_eq!(s.pop(), Some(0));
        assert!(s.is_empty());
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn removing_a_sheet_keeps_the_others_in_order() {
        let mut s = stack(3);
        assert_eq!(s.remove(|v| *v == 1), Some(1));
        assert_eq!(depths(&s), vec![(1, 0), (0, 2)]);
        assert_eq!(s.last_change(), Change::Pushed, "the top didn't change");
        assert_eq!(s.remove(|v| *v == 2), Some(2));
        assert_eq!(s.last_change(), Change::Popped(1));
        assert_eq!(s.remove(|v| *v == 7), None);
    }

    #[test]
    fn clearing_takes_every_sheet() {
        let mut s = stack(3);
        assert_eq!(s.clear(), vec![0, 1, 2]);
        assert!(s.is_empty());
        assert_eq!(s.last_change(), Change::Popped(3));
    }

    #[test]
    fn only_the_sheets_with_a_visible_edge_are_drawn_deepest_first() {
        assert_eq!(depths(&stack(0)), vec![]);
        assert_eq!(depths(&stack(2)), vec![(1, 0), (0, 1)]);
        let full = depths(&stack(MAX_SHEETS));
        assert_eq!(full.len(), MAX_PEEKS + 1);
        assert_eq!(full.first(), Some(&(MAX_PEEKS, MAX_SHEETS - 1 - MAX_PEEKS)));
        assert_eq!(full.last(), Some(&(0, MAX_SHEETS - 1)));
    }

    #[test]
    fn lower_sheets_shift_left_up_to_the_last_edge() {
        assert_eq!(offset(0), 0.);
        assert_eq!(offset(1), PEEK);
        assert_eq!(offset(2), 2. * PEEK);
        assert_eq!(offset(MAX_PEEKS + 4), offset(MAX_PEEKS));
    }

    #[test]
    fn sheets_know_where_they_moved_from() {
        let mut s = stack(3);
        assert_eq!(s.moved_from(0), None, "the new top slides in");
        assert_eq!(s.moved_from(1), Some(0));
        s.pop();
        assert_eq!(s.moved_from(0), Some(1));
    }
}
