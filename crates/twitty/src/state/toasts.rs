//! Short messages at the bottom of the window ("sonner"): what an action
//! did, or why it failed. Successes are green and errors red; each one hides
//! by itself after a while and fades out.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use twitty_model::{BackendError, FollowAction, PostAction};

use crate::state::compose::ComposeTarget;

/// How long a success stays up.
pub const SUCCESS_TTL: Duration = Duration::from_millis(3000);
/// How long an error stays up: long enough to read X's reason.
pub const ERROR_TTL: Duration = Duration::from_millis(6000);
/// How long a toast takes to fade out once it's done.
pub const EXIT: Duration = Duration::from_millis(200);
/// At most this many are shown; a new one pushes the oldest out.
pub const MAX_VISIBLE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

/// What to show, before it's on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: ToastKind,
    pub text: String,
}

impl Notice {
    pub fn success(text: impl Into<String>) -> Self {
        Self { kind: ToastKind::Success, text: text.into() }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { kind: ToastKind::Error, text: text.into() }
    }

    /// What a post action that went through says. Likes say nothing: the
    /// heart already shows it, and a toast per like would be noise.
    pub fn action_done(action: &PostAction) -> Option<Self> {
        let text = match action {
            PostAction::Like(_) | PostAction::Unlike(_) => return None,
            PostAction::Repost(_) => "Reposted",
            PostAction::Unrepost(_) => "Repost undone",
            PostAction::Bookmark(_) => "Added to Bookmarks",
            PostAction::Unbookmark(_) => "Removed from Bookmarks",
        };
        Some(Self::success(text))
    }

    /// Why a post action was undone.
    pub fn action_failed(action: &PostAction, error: &BackendError, now: DateTime<Utc>) -> Self {
        let what = match action {
            PostAction::Like(_) => "like",
            PostAction::Unlike(_) => "unlike",
            PostAction::Repost(_) => "repost",
            PostAction::Unrepost(_) => "undo the repost",
            PostAction::Bookmark(_) => "bookmark",
            PostAction::Unbookmark(_) => "remove the bookmark",
        };
        Self::error(format!("Couldn't {what}: {}", reason(error, now)))
    }

    /// A post, reply or quote of `target` was sent.
    pub fn published(target: &ComposeTarget) -> Self {
        Self::success(match target {
            ComposeTarget::New => "Post sent",
            ComposeTarget::Reply(_) => "Reply sent",
            ComposeTarget::Quote(_) => "Quote sent",
        })
    }

    /// A follow change went through, for the account `@handle`.
    pub fn follow_done(action: FollowAction, handle: &str) -> Self {
        match action {
            FollowAction::Follow => Self::success(format!("Following @{handle}")),
            FollowAction::Unfollow => Self::success(format!("Unfollowed @{handle}")),
        }
    }

    /// A follow change failed.
    pub fn follow_failed(action: FollowAction, error: &BackendError, now: DateTime<Utc>) -> Self {
        let what = match action {
            FollowAction::Follow => "follow",
            FollowAction::Unfollow => "unfollow",
        };
        Self::error(format!("Couldn't {what}: {}", reason(error, now)))
    }
}

/// A backend error in words for a toast.
pub fn reason(error: &BackendError, now: DateTime<Utc>) -> String {
    match error {
        BackendError::Unauthorized => "your session expired, sign in again".into(),
        BackendError::RateLimited { reset_at } => {
            let secs = u64::try_from((*reset_at - now).num_seconds()).unwrap_or(0);
            let minutes = secs.div_ceil(60).max(1);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            format!("rate limited, try again in {minutes} {unit}")
        }
        BackendError::NotFound => "it no longer exists".into(),
        BackendError::Network(_) => "no connection to the server".into(),
        BackendError::Upstream(message) => message.clone(),
        BackendError::Decode(_) => "unexpected answer from the server".into(),
    }
}

/// One message on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// Unique for the window's lifetime; keys the toast's animations.
    pub id: u64,
    pub kind: ToastKind,
    pub text: String,
    /// When it hides (then fades out for [`EXIT`]).
    pub hides_at: Instant,
}

impl Toast {
    /// Whether it is fading out at `now`.
    pub fn is_leaving(&self, now: Instant) -> bool {
        now >= self.hides_at
    }
}

/// The toasts of a window, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toasts {
    items: Vec<Toast>,
    next_id: u64,
}

impl Toasts {
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Shows `notice`. The same message already up is shown again for its
    /// full time instead of twice.
    pub fn push(&mut self, notice: Notice, now: Instant) -> u64 {
        let ttl = match notice.kind {
            ToastKind::Success => SUCCESS_TTL,
            ToastKind::Error => ERROR_TTL,
        };
        let hides_at = now + ttl;
        if let Some(same) = self
            .items
            .iter_mut()
            .find(|t| !t.is_leaving(now) && t.kind == notice.kind && t.text == notice.text)
        {
            same.hides_at = hides_at;
            return same.id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Toast { id, kind: notice.kind, text: notice.text, hides_at });
        // Too many: the oldest still shown starts leaving now.
        let mut shown: Vec<&mut Toast> =
            self.items.iter_mut().filter(|t| !t.is_leaving(now)).collect();
        let excess = shown.len().saturating_sub(MAX_VISIBLE);
        for toast in shown.iter_mut().take(excess) {
            toast.hides_at = now;
        }
        id
    }

    /// Starts the fade-out of toast `id` (a click on it).
    pub fn dismiss(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id && !t.is_leaving(now)) {
            t.hides_at = now;
        }
    }

    /// Drops the toasts that finished fading out. Returns whether any did.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items.retain(|t| now < t.hides_at + EXIT);
        self.items.len() != before
    }

    /// The next moment something changes (a toast starts or ends its fade).
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        self.items
            .iter()
            .map(|t| if t.is_leaving(now) { t.hides_at + EXIT } else { t.hides_at })
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use twitty_model::PostId;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn toasts_hide_then_fade_out_then_go() {
        let t0 = Instant::now();
        let mut toasts = Toasts::default();
        toasts.push(Notice::success("Reposted"), t0);
        toasts.push(Notice::error("Couldn't like: nope"), t0);
        assert_eq!(toasts.next_change(t0), Some(t0 + SUCCESS_TTL));

        let hidden = t0 + SUCCESS_TTL;
        assert!(toasts.items()[0].is_leaving(hidden));
        assert!(!toasts.items()[1].is_leaving(hidden), "errors stay up longer");
        assert!(!toasts.expire(hidden), "still fading out");
        assert_eq!(toasts.next_change(hidden), Some(hidden + EXIT));

        assert!(toasts.expire(hidden + EXIT));
        assert_eq!(toasts.items().len(), 1);
        assert_eq!(toasts.items()[0].kind, ToastKind::Error);
        assert!(toasts.expire(t0 + ERROR_TTL + EXIT));
        assert_eq!(toasts.next_change(t0 + ERROR_TTL + EXIT), None);
    }

    #[test]
    fn the_same_message_is_shown_once_and_kept_up() {
        let t0 = Instant::now();
        let mut toasts = Toasts::default();
        let a = toasts.push(Notice::success("Reposted"), t0);
        let b = toasts.push(Notice::success("Reposted"), t0 + ms(1000));
        assert_eq!(a, b);
        assert_eq!(toasts.items().len(), 1);
        assert_eq!(toasts.items()[0].hides_at, t0 + ms(1000) + SUCCESS_TTL);
        assert_ne!(toasts.push(Notice::error("Reposted"), t0), a, "another kind is another toast");
    }

    #[test]
    fn a_new_toast_pushes_the_oldest_out() {
        let t0 = Instant::now();
        let mut toasts = Toasts::default();
        for i in 0..MAX_VISIBLE + 1 {
            toasts.push(Notice::success(format!("n{i}")), t0);
        }
        let leaving: Vec<bool> = toasts.items().iter().map(|t| t.is_leaving(t0)).collect();
        assert_eq!(leaving, [true, false, false, false]);
        assert!(toasts.expire(t0 + EXIT));
        assert_eq!(toasts.items()[0].text, "n1");
    }

    #[test]
    fn a_click_dismisses() {
        let t0 = Instant::now();
        let mut toasts = Toasts::default();
        let id = toasts.push(Notice::error("x"), t0);
        toasts.dismiss(id, t0 + ms(10));
        assert!(toasts.items()[0].is_leaving(t0 + ms(10)));
        assert!(toasts.expire(t0 + ms(10) + EXIT));
        assert!(toasts.items().is_empty());
    }

    #[test]
    fn action_messages() {
        let id = PostId::new("1");
        assert_eq!(Notice::action_done(&PostAction::Like(id.clone())), None);
        assert_eq!(
            Notice::action_done(&PostAction::Repost(id.clone())),
            Some(Notice::success("Reposted"))
        );
        assert_eq!(
            Notice::action_done(&PostAction::Unbookmark(id.clone())),
            Some(Notice::success("Removed from Bookmarks"))
        );
        let now = Utc::now();
        assert_eq!(
            Notice::action_failed(
                &PostAction::Repost(id),
                &BackendError::Upstream("X error 226: automated".into()),
                now
            ),
            Notice::error("Couldn't repost: X error 226: automated")
        );
    }

    #[test]
    fn reasons_are_readable() {
        let now = Utc::now();
        let limited =
            |secs| BackendError::RateLimited { reset_at: now + chrono::Duration::seconds(secs) };
        assert_eq!(reason(&limited(30), now), "rate limited, try again in 1 minute");
        assert_eq!(reason(&limited(150), now), "rate limited, try again in 3 minutes");
        assert_eq!(reason(&limited(-5), now), "rate limited, try again in 1 minute");
        assert_eq!(reason(&BackendError::Unauthorized, now), "your session expired, sign in again");
        assert_eq!(
            reason(&BackendError::Network("tls".into()), now),
            "no connection to the server"
        );
    }

    #[test]
    fn published_says_what_was_sent() {
        use twitty_model::{Backend, Draft, mock::MockBackend};
        let backend = MockBackend::new();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let post = rt.block_on(backend.publish(Draft::default())).unwrap();
        let post = std::sync::Arc::new(post);
        assert_eq!(Notice::published(&ComposeTarget::New), Notice::success("Post sent"));
        let reply = ComposeTarget::Reply(std::sync::Arc::clone(&post));
        assert_eq!(Notice::published(&reply), Notice::success("Reply sent"));
        let quote = ComposeTarget::Quote(post);
        assert_eq!(Notice::published(&quote), Notice::success("Quote sent"));
    }

    #[test]
    fn follow_messages() {
        assert_eq!(
            Notice::follow_done(FollowAction::Follow, "jack"),
            Notice::success("Following @jack")
        );
        assert_eq!(
            Notice::follow_failed(FollowAction::Unfollow, &BackendError::NotFound, Utc::now()),
            Notice::error("Couldn't unfollow: it no longer exists")
        );
    }
}
