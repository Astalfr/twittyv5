//! Keeping the counters of the tweets on screen current: which tweets to look
//! up, and when.
//!
//! Only rows the viewer can see are refreshed, in one request for the whole
//! deck. Rows scrolled away keep the counters they had; they are looked up
//! again when they come back into view.

use std::{collections::HashSet, ops::Range, time::Duration};

use chrono::{DateTime, Utc};
use twitty_model::{BackendError, FeedItem, PostId};

/// How often the counters of visible tweets are refreshed.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// Most tweets looked up per refresh.
pub const MAX_IDS: usize = 100;
/// Most rows of one column counted as visible, for a list that hasn't been
/// laid out yet (every row then looks visible).
pub const MAX_VISIBLE_ROWS: usize = 20;

/// The rows on screen: from the top row down to the first one entirely below
/// the viewport. `below(ix)` says whether row `ix` is; `None` (not measured)
/// counts as visible.
pub fn visible_rows(top: usize, len: usize, below: impl Fn(usize) -> Option<bool>) -> Range<usize> {
    let start = top.min(len);
    let end = (start..len)
        .take(MAX_VISIBLE_ROWS)
        .find(|&ix| below(ix) == Some(true))
        .unwrap_or_else(|| (start + MAX_VISIBLE_ROWS).min(len));
    start..end
}

/// The tweets of `rows` whose counters are shown, without duplicates (the
/// same tweet in two columns), in order, at most [`MAX_IDS`].
pub fn ids<'a>(rows: impl IntoIterator<Item = &'a FeedItem>) -> Vec<PostId> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .flat_map(FeedItem::posts)
        .filter(|post| seen.insert(&post.id))
        .map(|post| post.id.clone())
        .take(MAX_IDS)
        .collect()
}

/// How long to wait before the next refresh, after one that ended with
/// `result` at `now`: until a rate limit resets, else [`REFRESH_EVERY`].
pub fn next_delay(result: Result<(), &BackendError>, now: DateTime<Utc>) -> Duration {
    match result {
        Err(BackendError::RateLimited { reset_at }) => {
            (*reset_at - now).to_std().unwrap_or_default().max(REFRESH_EVERY)
        }
        _ => REFRESH_EVERY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use twitty_model::{FeedSource, HomeMode, PageRequest, Post, mock::MockBackend};

    fn rows() -> Vec<FeedItem> {
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(twitty_model::Backend::feed(
                &backend,
                &FeedSource::Home { mode: HomeMode::Following },
                PageRequest::First,
            ))
            .unwrap();
        page.items
    }

    #[test]
    fn visible_rows_stop_at_the_first_row_below_the_viewport() {
        assert_eq!(visible_rows(3, 50, |ix| Some(ix >= 7)), 3..7);
        assert_eq!(visible_rows(0, 4, |_| Some(false)), 0..4, "short list");
        assert_eq!(visible_rows(9, 4, |_| Some(false)), 4..4, "top past the end");
    }

    #[test]
    fn unmeasured_rows_count_as_visible_up_to_a_cap() {
        assert_eq!(visible_rows(5, 100, |_| None), 5..5 + MAX_VISIBLE_ROWS);
    }

    #[test]
    fn ids_take_every_shown_tweet_once() {
        let rows = rows();
        let posts: Vec<&Post> = rows.iter().flat_map(FeedItem::posts).collect();
        // The same rows in two columns: one lookup each.
        let ids = ids(rows.iter().chain(&rows));
        let expected: Vec<PostId> = posts.iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn ids_of_a_retweet_are_the_original_tweet() {
        let rows = rows();
        let (by, post) = rows
            .iter()
            .find_map(|row| match row {
                FeedItem::Repost { by, post } => Some((Arc::clone(by), post.clone())),
                _ => None,
            })
            .expect("the mock has retweets");
        assert_ne!(by.id, post.author.id);
        assert_eq!(ids([&FeedItem::Repost { by, post: post.clone() }]), vec![post.id]);
    }

    #[test]
    fn ids_are_capped() {
        let rows = rows();
        let many = std::iter::repeat_n(&rows, 20).flatten().enumerate().map(|(n, row)| {
            let mut row = row.clone();
            if let FeedItem::Post(p) = &mut row {
                p.id = PostId::new(format!("copy-{n}"));
            }
            row
        });
        let many: Vec<FeedItem> = many.collect();
        assert_eq!(ids(&many).len(), MAX_IDS);
    }

    #[test]
    fn rate_limits_push_the_next_refresh_to_their_reset() {
        let now = Utc::now();
        let limited = BackendError::RateLimited { reset_at: now + chrono::Duration::minutes(10) };
        assert_eq!(next_delay(Err(&limited), now), Duration::from_secs(600));
        let soon = BackendError::RateLimited { reset_at: now + chrono::Duration::seconds(5) };
        assert_eq!(next_delay(Err(&soon), now), REFRESH_EVERY);
        assert_eq!(next_delay(Ok(()), now), REFRESH_EVERY);
        assert_eq!(next_delay(Err(&BackendError::Network("x".into())), now), REFRESH_EVERY);
    }
}
