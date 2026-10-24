//! Optimistic like / repost / bookmark state shared by every column.
//!
//! The viewer's own changes are kept as overrides on top of what the backend
//! last returned, so a like shows up instantly in every column and can be
//! rolled back if the request fails.

use std::collections::HashMap;

use twitty_model::{Metrics, Post, PostAction, PostId, ViewerPostState};

/// A toggleable viewer action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Toggle {
    Like,
    Repost,
    Bookmark,
}

/// The viewer state and counters to display for a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub viewer: ViewerPostState,
    pub metrics: Metrics,
}

/// A change that was applied optimistically and may need to be undone.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub action: PostAction,
    post: PostId,
    previous: Option<Effective>,
}

/// Overrides keyed by post id.
#[derive(Debug, Default)]
pub struct Interactions {
    overrides: HashMap<PostId, Effective>,
}

impl Interactions {
    /// State to render for `post`.
    pub fn effective(&self, post: &Post) -> Effective {
        self.overrides
            .get(&post.id)
            .copied()
            .unwrap_or(Effective { viewer: post.viewer, metrics: post.metrics })
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

    /// Undoes a failed optimistic change.
    pub fn rollback(&mut self, pending: Pending) {
        match pending.previous {
            Some(prev) => self.overrides.insert(pending.post, prev),
            None => self.overrides.remove(&pending.post),
        };
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
