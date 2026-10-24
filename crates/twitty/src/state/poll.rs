//! How often each column checks for new posts, within the provider's limits.
//!
//! Polling may use a third of a feed's budget; the rest stays for scrolling,
//! refreshes and other columns' first loads. Columns that hit the same feed
//! type share that third.

use std::time::Duration;

use twitty_model::{FeedSource, HomeMode, RateBudget, UserTab};

/// Never poll more often than this.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Never wait longer than this (a feed with a tiny budget still updates).
pub const MAX_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Part of the budget polling may use: 1 / `POLL_SHARE`.
const POLL_SHARE: u32 = 3;

/// Feeds that are likely served by the same request type, and so share a
/// rate limit. Conversations are not polled at all.
pub fn bucket(source: &FeedSource) -> Option<&'static str> {
    Some(match source {
        FeedSource::Home { mode: HomeMode::ForYou } => "home",
        FeedSource::Home { mode: HomeMode::Following } => "home-latest",
        FeedSource::Notifications { .. } => "notifications",
        FeedSource::Search { .. } => "search",
        FeedSource::User { tab: UserTab::Posts, .. } => "user-posts",
        FeedSource::User { tab: UserTab::Replies, .. } => "user-replies",
        FeedSource::User { tab: UserTab::Media, .. } => "user-media",
        FeedSource::User { tab: UserTab::Likes, .. } => "user-likes",
        FeedSource::List { .. } => "list",
        FeedSource::Bookmarks => "bookmarks",
        FeedSource::Thread { .. } => return None,
    })
}

/// The polling interval of one of `sharing` columns using a feed with `budget`.
pub fn interval(budget: Option<RateBudget>, sharing: usize) -> Duration {
    let Some(budget) = budget else { return MIN_INTERVAL };
    let allowed = (budget.limit / POLL_SHARE).max(1);
    let sharing = u32::try_from(sharing.max(1)).unwrap_or(u32::MAX);
    (budget.window.saturating_mul(sharing) / allowed).clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Intervals for every column (`None` = don't poll), given the sources in
/// deck order and a budget lookup.
pub fn intervals(
    sources: &[FeedSource],
    budget: impl Fn(&FeedSource) -> Option<RateBudget>,
) -> Vec<Option<Duration>> {
    sources
        .iter()
        .map(|source| {
            let b = bucket(source)?;
            let sharing = sources.iter().filter(|s| bucket(s) == Some(b)).count();
            Some(interval(budget(source), sharing))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{NotificationFilter, PostId, SearchSort};

    const WINDOW: Duration = Duration::from_secs(900);

    fn budget(limit: u32) -> Option<RateBudget> {
        Some(RateBudget { limit, window: WINDOW })
    }

    #[test]
    fn generous_budgets_poll_at_the_minimum() {
        assert_eq!(interval(budget(500), 1), MIN_INTERVAL);
        assert_eq!(interval(None, 10), MIN_INTERVAL, "no limit");
    }

    #[test]
    fn shared_and_small_budgets_slow_down() {
        // 50 / 3 = 16 polls per 15 minutes for 6 columns: one every 337.5s each.
        assert_eq!(interval(budget(50), 6), Duration::from_millis(337_500));
        assert!(interval(budget(50), 6) > interval(budget(50), 1));
        assert_eq!(interval(budget(1), 1000), MAX_INTERVAL);
    }

    #[test]
    fn polling_stays_within_a_third_of_the_budget() {
        for limit in [3, 50, 180, 500] {
            for sharing in 1..12usize {
                let every = interval(budget(limit), sharing);
                let per_window = WINDOW.as_secs_f64() / every.as_secs_f64() * sharing as f64;
                let cap = (f64::from(limit) / 3.0).max(1.0);
                assert!(
                    per_window <= cap + 1e-9 || every == MIN_INTERVAL || every == MAX_INTERVAL,
                    "{limit}/{sharing}: {per_window}"
                );
            }
        }
    }

    #[test]
    fn columns_of_the_same_feed_type_share() {
        let sources = vec![
            FeedSource::Bookmarks,
            FeedSource::Bookmarks,
            FeedSource::Search { query: "a".into(), sort: SearchSort::Latest },
            FeedSource::Notifications { filter: NotificationFilter::All },
            FeedSource::Thread { root: PostId::new("1") },
        ];
        let got = intervals(&sources, |_| budget(30));
        // 30 / 3 = 10 polls: two bookmarks columns get 180s each, the others 90s.
        assert_eq!(
            got,
            vec![
                Some(Duration::from_secs(180)),
                Some(Duration::from_secs(180)),
                Some(Duration::from_secs(90)),
                Some(Duration::from_secs(90)),
                None,
            ]
        );
    }
}
