//! The profile sheet's Follow button: what it shows, what a click asks the
//! backend, and how the answer (or a failure) lands.

use twitty_model::{BackendError, FollowAction, FollowStatus, Profile};

/// Where the button stands.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    /// As the profile says; a refreshed profile replaces it.
    FromProfile(FollowStatus),
    /// A click is on its way; shown as if it had worked.
    Sending { action: FollowAction, before: FollowStatus },
    /// The backend answered a click. A refresh that started earlier may
    /// still carry the old relation, so it no longer changes this.
    Answered(FollowStatus),
}

/// The Follow button of one profile sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Follow {
    phase: Phase,
    /// Why the last click failed, until the next one.
    error: Option<String>,
}

impl Follow {
    pub fn new(profile: &Profile) -> Self {
        Self { phase: Phase::FromProfile(from_profile(profile)), error: None }
    }

    /// The status to show.
    pub fn status(&self) -> FollowStatus {
        match &self.phase {
            Phase::FromProfile(s) | Phase::Answered(s) => *s,
            Phase::Sending { action: FollowAction::Follow, .. } => FollowStatus::Following,
            Phase::Sending { action: FollowAction::Unfollow, .. } => FollowStatus::NotFollowing,
        }
    }

    pub fn is_sending(&self) -> bool {
        matches!(self.phase, Phase::Sending { .. })
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The button's text.
    pub fn label(&self) -> &'static str {
        match self.status() {
            FollowStatus::NotFollowing => "Follow",
            FollowStatus::Following => "Following",
            FollowStatus::Requested => "Requested",
        }
    }

    /// Whether a click does something. A request waits for the account's
    /// approval and can't be taken back from here.
    pub fn is_clickable(&self) -> bool {
        !self.is_sending() && self.status() != FollowStatus::Requested
    }

    /// A click: the action to send, or `None` when there is nothing to do.
    pub fn click(&mut self) -> Option<FollowAction> {
        if !self.is_clickable() {
            return None;
        }
        let before = self.status();
        let action = match before {
            FollowStatus::NotFollowing => FollowAction::Follow,
            FollowStatus::Following | FollowStatus::Requested => FollowAction::Unfollow,
        };
        self.phase = Phase::Sending { action, before };
        self.error = None;
        Some(action)
    }

    /// The backend's answer to the click in flight.
    pub fn answered(&mut self, result: Result<FollowStatus, BackendError>) {
        let Phase::Sending { action, before } = self.phase.clone() else { return };
        self.phase = match result {
            Ok(status) => Phase::Answered(status),
            Err(e) => {
                self.error = Some(failure(action, &e));
                // Back to what was shown; still the profile's view unless an
                // earlier click was answered.
                Phase::FromProfile(before)
            }
        };
    }

    /// A refreshed copy of the profile.
    pub fn refreshed(&mut self, profile: &Profile) {
        if let Phase::FromProfile(s) = &mut self.phase {
            *s = from_profile(profile);
        }
    }
}

fn from_profile(profile: &Profile) -> FollowStatus {
    if profile.viewer.following { FollowStatus::Following } else { FollowStatus::NotFollowing }
}

/// The line shown under the button after a failed click.
fn failure(action: FollowAction, e: &BackendError) -> String {
    let what = match action {
        FollowAction::Follow => "follow",
        FollowAction::Unfollow => "unfollow",
    };
    match e {
        // Twitter's own reason, e.g. "Twitter error 162: You have been blocked…".
        BackendError::Upstream(reason) => format!("Couldn't {what}: {reason}"),
        other => format!("Couldn't {what}: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::UserId;

    fn profile(following: bool) -> Profile {
        let mut p = Profile::new(UserId::new("1"), "someone", "Someone");
        p.viewer.following = following;
        p
    }

    #[test]
    fn follows_then_unfollows() {
        let mut f = Follow::new(&profile(false));
        assert_eq!(f.label(), "Follow");
        assert_eq!(f.click(), Some(FollowAction::Follow));
        assert_eq!(f.label(), "Following", "shown at once");
        assert!(f.is_sending() && f.click().is_none(), "one request at a time");
        f.answered(Ok(FollowStatus::Following));
        assert_eq!(f.click(), Some(FollowAction::Unfollow));
        f.answered(Ok(FollowStatus::NotFollowing));
        assert_eq!(f.status(), FollowStatus::NotFollowing);
    }

    #[test]
    fn a_failure_rolls_back_and_says_why() {
        let mut f = Follow::new(&profile(false));
        f.click();
        f.answered(Err(BackendError::Upstream("blocked".into())));
        assert_eq!(f.status(), FollowStatus::NotFollowing);
        assert_eq!(f.error(), Some("Couldn't follow: blocked"));
        f.click();
        assert_eq!(f.error(), None, "a new click clears it");
        f.answered(Err(BackendError::Network("timed out".into())));
        assert_eq!(f.error(), Some("Couldn't follow: network error: timed out"));
    }

    #[test]
    fn a_protected_account_is_requested_and_stays_so() {
        let mut f = Follow::new(&profile(false));
        f.click();
        f.answered(Ok(FollowStatus::Requested));
        assert_eq!(f.label(), "Requested");
        assert!(!f.is_clickable() && f.click().is_none());
        f.refreshed(&profile(false));
        assert_eq!(f.status(), FollowStatus::Requested, "a refresh doesn't know about requests");
    }

    #[test]
    fn a_refresh_after_a_click_does_not_undo_it() {
        let mut f = Follow::new(&profile(false));
        f.click();
        // The refresh was sent before the click and answers after it.
        f.refreshed(&profile(false));
        assert_eq!(f.status(), FollowStatus::Following);
        f.answered(Ok(FollowStatus::Following));
        f.refreshed(&profile(false));
        assert_eq!(f.status(), FollowStatus::Following);
    }

    #[test]
    fn a_refresh_before_any_click_updates_the_button() {
        let mut f = Follow::new(&profile(false));
        f.refreshed(&profile(true));
        assert_eq!(f.label(), "Following");
    }

    #[test]
    fn answers_without_a_click_are_ignored() {
        let mut f = Follow::new(&profile(true));
        f.answered(Ok(FollowStatus::NotFollowing));
        assert_eq!(f.status(), FollowStatus::Following);
    }
}
