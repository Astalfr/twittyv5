//! The "Search people" dialog: how what the user typed becomes a query, and
//! where the search stands. The search follows the input: it runs once typing
//! has paused for [`DEBOUNCE`], so a word typed quickly costs one request of
//! the rate budget it shares with the search columns.

use std::{sync::Arc, time::Duration};

use twitty_model::{BackendError, Profile};

/// How long the input must stay unchanged before it is searched.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// The query for `input`: trimmed, without a leading `@` (Twitter matches handles
/// without it). `None` when nothing is left to search.
pub fn query(input: &str) -> Option<String> {
    let q = input.trim();
    let q = q.strip_prefix('@').unwrap_or(q).trim();
    (!q.is_empty()).then(|| q.to_owned())
}

/// Where the search stands.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    /// Waiting for `query`; the last results stay shown meanwhile.
    Searching {
        query: String,
        previous: Vec<Arc<Profile>>,
    },
    Done {
        query: String,
        users: Vec<Arc<Profile>>,
    },
    Failed(String),
}

/// One search at a time; an answer to an older search is ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct UserSearch {
    phase: Phase,
    /// Bumped by each search (and by clearing), so a late answer can tell it
    /// is stale.
    generation: u64,
}

impl Default for UserSearch {
    fn default() -> Self {
        Self { phase: Phase::Idle, generation: 0 }
    }
}

impl UserSearch {
    /// Follows the input once it settled. Returns the query to send and the
    /// token to hand back to [`UserSearch::finish`], or `None` when nothing
    /// needs sending: the input is empty (the results are cleared) or it
    /// asks for what is already searched or shown.
    pub fn start(&mut self, input: &str) -> Option<(u64, String)> {
        self.generation += 1;
        let Some(q) = query(input) else {
            self.phase = Phase::Idle;
            return None;
        };
        let previous = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Searching { query, previous } if query == q => {
                self.phase = Phase::Searching { query, previous };
                self.generation -= 1;
                return None;
            }
            Phase::Done { query, users } if query == q => {
                self.phase = Phase::Done { query, users };
                return None;
            }
            Phase::Searching { previous, .. } => previous,
            Phase::Done { users, .. } => users,
            Phase::Idle | Phase::Failed(_) => Vec::new(),
        };
        self.phase = Phase::Searching { query: q.clone(), previous };
        Some((self.generation, q))
    }

    /// Records the answer to search `generation`, unless a newer one started.
    pub fn finish(&mut self, generation: u64, result: Result<Vec<Profile>, BackendError>) {
        if generation != self.generation {
            return;
        }
        let Phase::Searching { query, .. } = &self.phase else { return };
        self.phase = match result {
            Ok(users) => Phase::Done {
                query: query.clone(),
                users: users.into_iter().map(Arc::new).collect(),
            },
            Err(e) => Phase::Failed(format!("Couldn't search for “{query}”: {e}")),
        };
    }

    pub fn is_searching(&self) -> bool {
        matches!(self.phase, Phase::Searching { .. })
    }

    /// The accounts to show: the last search's, also while the next one runs.
    pub fn results(&self) -> &[Arc<Profile>] {
        match &self.phase {
            Phase::Done { users, .. } => users,
            Phase::Searching { previous, .. } => previous,
            Phase::Idle | Phase::Failed(_) => &[],
        }
    }

    /// What to show instead of results: an error, or that nothing matched.
    pub fn message(&self) -> Option<String> {
        match &self.phase {
            Phase::Failed(e) => Some(e.clone()),
            Phase::Done { query, users } if users.is_empty() => {
                Some(format!("No accounts match “{query}”."))
            }
            _ => None,
        }
    }

    /// Whether [`UserSearch::message`] is an error.
    pub fn failed(&self) -> bool {
        matches!(self.phase, Phase::Failed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use twitty_model::UserId;

    fn ada() -> Profile {
        Profile::new(UserId::new("1"), "ada", "Ada Lovelace")
    }

    fn handles(s: &UserSearch) -> Vec<String> {
        s.results().iter().map(|p| p.handle.to_string()).collect()
    }

    #[test]
    fn queries_are_trimmed_and_lose_the_at_sign() {
        assert_eq!(query("  ada lovelace "), Some("ada lovelace".into()));
        assert_eq!(query("@NASA"), Some("NASA".into()));
        assert_eq!(query(" @ "), None);
        assert_eq!(query(""), None);
    }

    #[test]
    fn results_land_and_nothing_found_says_so() {
        let mut s = UserSearch::default();
        let (token, q) = s.start("@ada").unwrap();
        assert_eq!(q, "ada");
        assert!(s.is_searching());
        s.finish(token, Ok(vec![ada()]));
        assert_eq!(handles(&s), ["ada"]);
        assert_eq!(s.message(), None);

        let (token, _) = s.start("zzz").unwrap();
        assert_eq!(handles(&s), ["ada"], "the last results stay while searching");
        s.finish(token, Ok(Vec::new()));
        assert!(s.results().is_empty());
        assert_eq!(s.message().as_deref(), Some("No accounts match “zzz”."));
        assert!(!s.failed());
    }

    #[test]
    fn clearing_the_input_clears_the_results_and_drops_the_pending_answer() {
        let mut s = UserSearch::default();
        let (token, _) = s.start("ada").unwrap();
        assert_eq!(s.start("  @ "), None);
        s.finish(token, Ok(vec![ada()]));
        assert!(s.results().is_empty());
        assert_eq!(s.message(), None);
        assert!(!s.is_searching());
    }

    #[test]
    fn the_same_query_is_not_sent_twice() {
        let mut s = UserSearch::default();
        let (token, _) = s.start("ada").unwrap();
        assert_eq!(s.start("ada "), None, "while it runs");
        s.finish(token, Ok(vec![ada()]));
        assert_eq!(s.start("@ada"), None, "once it is shown");
        assert_eq!(handles(&s), ["ada"]);
    }

    #[test]
    fn a_failure_is_shown_and_can_be_retried() {
        let mut s = UserSearch::default();
        let (token, _) = s.start("ada").unwrap();
        s.finish(token, Err(BackendError::Network("down".into())));
        assert!(s.failed());
        assert!(s.message().is_some_and(|m| m.starts_with("Couldn't search for “ada”")));
        assert!(s.start("ada").is_some());
    }

    #[test]
    fn a_late_answer_to_an_older_search_is_ignored() {
        let mut s = UserSearch::default();
        let (first, _) = s.start("ada").unwrap();
        let (second, _) = s.start("grace").unwrap();
        s.finish(first, Ok(vec![ada()]));
        assert!(s.is_searching(), "still waiting for the newer search");
        s.finish(second, Ok(Vec::new()));
        assert_eq!(s.message().as_deref(), Some("No accounts match “grace”."));
    }
}
