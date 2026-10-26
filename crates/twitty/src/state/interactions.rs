//! Optimistic like / retweet / bookmark state, and the latest counters,
//! shared by every column.
//!
//! What to show for a tweet comes from, in order:
//! 1. the viewer's own change while it is being sent, so a like shows up
//!    instantly in every column and can be rolled back if the request fails;
//! 2. the latest state the backend reported for it, from any load or stats
//!    refresh, so counters keep moving after the tweet was loaded;
//! 3. the tweet as its row holds it.

use std::collections::HashMap;

use twitty_model::{Metrics, Post, PostAction, PostId, ViewerPostState};

/// A toggleable viewer action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Toggle {
    Like,
    Repost,
    Bookmark,
}

/// The viewer state and counters to display for a tweet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub viewer: ViewerPostState,
    pub metrics: Metrics,
}

impl Effective {
    /// Whether `toggle` is on (liked, retweeted, bookmarked).
    pub fn is_on(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::Like => self.viewer.liked,
            Toggle::Repost => self.viewer.reposted,
            Toggle::Bookmark => self.viewer.bookmarked,
        }
    }

    fn of(post: &Post) -> Self {
        Self { viewer: post.viewer, metrics: post.metrics }
    }
}

/// A change that was applied optimistically and may need to be undone.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub action: PostAction,
    post: PostId,
    previous: Option<Effective>,
}

/// The viewer's changes to one tweet that are being sent.
#[derive(Debug, Default)]
struct InFlight {
    /// How many are waiting for the backend.
    waiting: u32,
    /// Whether the backend accepted one of them already.
    confirmed: bool,
}

/// Per-tweet state on top of what the rows hold, keyed by tweet id.
#[derive(Debug, Default)]
pub struct Interactions {
    /// The viewer's changes still being sent.
    overrides: HashMap<PostId, Effective>,
    /// Changes to each tweet being sent. While any is, reported state is
    /// ignored: it may predate the change.
    in_flight: HashMap<PostId, InFlight>,
    /// The latest state the backend reported.
    reported: HashMap<PostId, Effective>,
}

impl Interactions {
    /// State to render for `post`.
    pub fn effective(&self, post: &Post) -> Effective {
        self.overrides
            .get(&post.id)
            .or_else(|| self.reported.get(&post.id))
            .copied()
            .unwrap_or_else(|| Effective::of(post))
    }

    /// Remembers `post`'s state as the backend just reported it, unless the
    /// viewer is changing it. Returns whether it is news: rows holding an
    /// older copy of the tweet may show something else now.
    pub fn record(&mut self, post: &Post) -> bool {
        if self.in_flight.contains_key(&post.id) {
            return false;
        }
        let now = Effective::of(post);
        self.reported.insert(post.id.clone(), now) != Some(now)
    }

    /// Flips `toggle` on `post` locally and returns the backend action to send.
    pub fn toggle(&mut self, post: &Post, toggle: Toggle) -> Pending {
        let previous = self.overrides.get(&post.id).copied();
        let mut next = self.effective(post);
        let (flag, counter) = match toggle {
            Toggle::Like => (&mut next.viewer.liked, &mut next.metrics.likes),
            Toggle::Repost => (&mut next.viewer.reposted, &mut next.metrics.reposts),
            Toggle::Bookmark => (&mut next.viewer.bookmarked, &mut next.metrics.bookmarks),
        };
        *flag = !*flag;
        *counter = if *flag { counter.saturating_add(1) } else { counter.saturating_sub(1) };
        let on = *flag;
        self.overrides.insert(post.id.clone(), next);
        self.in_flight.entry(post.id.clone()).or_default().waiting += 1;

        let id = post.id.clone();
        let action = match (toggle, on) {
            (Toggle::Like, true) => PostAction::Like(id),
            (Toggle::Like, false) => PostAction::Unlike(id),
            (Toggle::Repost, true) => PostAction::Repost(id),
            (Toggle::Repost, false) => PostAction::Unrepost(id),
            (Toggle::Bookmark, true) => PostAction::Bookmark(id),
            (Toggle::Bookmark, false) => PostAction::Unbookmark(id),
        };
        Pending { action, post: post.id.clone(), previous }
    }

    /// The backend accepted a change. Once none is left in flight, the
    /// optimistic state becomes the reported one, until the next report.
    pub fn confirm(&mut self, pending: Pending) {
        if self.settle(&pending.post, true).is_some()
            && let Some(state) = self.overrides.remove(&pending.post)
        {
            self.reported.insert(pending.post, state);
        }
    }

    /// Undoes a failed optimistic change.
    pub fn rollback(&mut self, pending: Pending) {
        let settled = self.settle(&pending.post, false);
        match (pending.previous, settled) {
            // Others are still being sent: back to the state before this one.
            (Some(prev), None) => {
                self.overrides.insert(pending.post, prev);
            }
            // The state before held changes the backend has since accepted
            // (and no failed one, which would have removed the override):
            // it is the reported state now, as in `confirm`.
            (Some(prev), Some(true)) => {
                self.overrides.remove(&pending.post);
                self.reported.insert(pending.post, prev);
            }
            // Nothing the viewer changed went through.
            (None, _) | (Some(_), Some(false)) => {
                self.overrides.remove(&pending.post);
            }
        }
    }

    /// Counts one change to `post` as answered (`accepted` or not). Once none
    /// is left, returns whether any of them was accepted.
    fn settle(&mut self, post: &PostId, accepted: bool) -> Option<bool> {
        let entry = self.in_flight.get_mut(post)?;
        entry.confirmed |= accepted;
        entry.waiting = entry.waiting.saturating_sub(1);
        if entry.waiting > 0 {
            return None;
        }
        self.in_flight.remove(post).map(|entry| entry.confirmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{Backend, FeedSource, HomeMode, PageRequest, mock::MockBackend};

    fn sample_post() -> Post {
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                backend.feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First),
            )
            .unwrap();
        page.items[0].posts()[0].clone()
    }

    #[test]
    fn like_then_unlike_round_trips_counts() {
        let post = sample_post();
        let base = post.metrics.likes;
        let mut i = Interactions::default();

        let p = i.toggle(&post, Toggle::Like);
        assert_eq!(p.action, PostAction::Like(post.id.clone()));
        assert!(i.effective(&post).viewer.liked);
        assert_eq!(i.effective(&post).metrics.likes, base + 1);

        let p = i.toggle(&post, Toggle::Like);
        assert_eq!(p.action, PostAction::Unlike(post.id.clone()));
        assert!(!i.effective(&post).viewer.liked);
        assert_eq!(i.effective(&post).metrics.likes, base);
    }

    #[test]
    fn toggles_are_independent() {
        let post = sample_post();
        let mut i = Interactions::default();
        i.toggle(&post, Toggle::Repost);
        i.toggle(&post, Toggle::Bookmark);
        let e = i.effective(&post);
        assert!(e.viewer.reposted && e.viewer.bookmarked && !e.viewer.liked);
    }

    #[test]
    fn rollback_restores_previous_state() {
        let post = sample_post();
        let mut i = Interactions::default();
        let before = i.effective(&post);
        let pending = i.toggle(&post, Toggle::Like);
        i.rollback(pending);
        assert_eq!(i.effective(&post), before);
    }

    #[test]
    fn rollback_of_second_change_keeps_first() {
        let post = sample_post();
        let mut i = Interactions::default();
        i.toggle(&post, Toggle::Like);
        let after_like = i.effective(&post);
        let pending = i.toggle(&post, Toggle::Repost);
        i.rollback(pending);
        assert_eq!(i.effective(&post), after_like);
    }

    fn with_metrics(post: &Post, likes: u64, views: u64) -> Post {
        let mut post = post.clone();
        post.metrics.likes = likes;
        post.metrics.views = Some(views);
        post
    }

    #[test]
    fn reported_state_replaces_what_the_row_holds() {
        let post = sample_post();
        let mut i = Interactions::default();
        assert!(i.record(&post), "first report");
        assert!(!i.record(&post), "nothing new");
        let fresh = with_metrics(&post, 500, 9000);
        assert!(i.record(&fresh));
        assert_eq!(i.effective(&post).metrics, fresh.metrics, "the old row shows new counters");

        let newer = with_metrics(&post, 501, 9500);
        assert!(i.record(&newer));
        assert_eq!(i.effective(&post).metrics, newer.metrics, "the latest report wins");
        assert!(!i.record(&newer), "nothing new");
    }

    #[test]
    fn reports_while_a_change_is_sent_are_ignored() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let pending = i.toggle(&post, Toggle::Like);
        // Fetched before the like reached the backend.
        assert!(!i.record(&with_metrics(&post, 10, 200)));
        assert!(i.effective(&post).viewer.liked);
        assert_eq!(i.effective(&post).metrics.likes, 11);

        i.confirm(pending);
        assert!(i.effective(&post).viewer.liked, "no flicker back after confirming");
        assert_eq!(i.effective(&post).metrics.likes, 11);
    }

    #[test]
    fn reports_after_a_confirmed_change_take_over() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let pending = i.toggle(&post, Toggle::Like);
        i.confirm(pending);

        let mut fresh = with_metrics(&post, 42, 300);
        fresh.viewer.liked = true;
        i.record(&fresh);
        assert_eq!(i.effective(&post), Effective::of(&fresh));
    }

    #[test]
    fn rollback_after_a_report_restores_the_report() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let fresh = with_metrics(&post, 20, 200);
        i.record(&fresh);
        let pending = i.toggle(&post, Toggle::Like);
        assert_eq!(i.effective(&post).metrics.likes, 21);

        i.rollback(pending);
        assert_eq!(i.effective(&post), Effective::of(&fresh));
        // Nothing in flight any more: reports apply again.
        let newer = with_metrics(&post, 30, 300);
        i.record(&newer);
        assert_eq!(i.effective(&post), Effective::of(&newer));
    }

    #[test]
    fn the_override_stays_until_every_change_is_answered() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let like = i.toggle(&post, Toggle::Like);
        let bookmark = i.toggle(&post, Toggle::Bookmark);
        i.confirm(like);
        i.record(&with_metrics(&post, 10, 200));
        assert!(i.effective(&post).viewer.bookmarked, "the bookmark is still being sent");

        i.confirm(bookmark);
        let e = i.effective(&post);
        assert!(e.viewer.liked && e.viewer.bookmarked);
    }

    #[test]
    fn a_failed_change_after_a_confirmed_one_lets_reports_through() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let like = i.toggle(&post, Toggle::Like);
        let bookmark = i.toggle(&post, Toggle::Bookmark);
        i.confirm(like);
        i.rollback(bookmark);
        let e = i.effective(&post);
        assert!(e.viewer.liked && !e.viewer.bookmarked, "the like stays, the bookmark doesn't");

        let mut fresh = with_metrics(&post, 42, 300);
        fresh.viewer.liked = true;
        assert!(i.record(&fresh));
        assert_eq!(i.effective(&post), Effective::of(&fresh), "counters keep moving");
    }

    #[test]
    fn changes_that_all_fail_leave_nothing_behind() {
        let post = with_metrics(&sample_post(), 10, 100);
        let mut i = Interactions::default();
        let before = i.effective(&post);
        let like = i.toggle(&post, Toggle::Like);
        let bookmark = i.toggle(&post, Toggle::Bookmark);
        // Offline: both fail, the later one last.
        i.rollback(like);
        i.rollback(bookmark);
        assert_eq!(i.effective(&post), before, "the failed like isn't shown as done");

        let fresh = with_metrics(&post, 42, 300);
        assert!(i.record(&fresh));
        assert_eq!(i.effective(&post), Effective::of(&fresh));
    }

    #[test]
    fn unliking_zero_count_saturates() {
        let mut post = sample_post();
        post.viewer.liked = true;
        post.metrics.likes = 0;
        let mut i = Interactions::default();
        i.toggle(&post, Toggle::Like);
        assert_eq!(i.effective(&post).metrics.likes, 0);
    }
}
