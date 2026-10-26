//! Shared, observable wrapper around [`Interactions`] that also sends the
//! actions to the backend, rolls back on failure, and reports both outcomes
//! as [`Notice`]s for the app's toasts.

use std::{sync::Arc, time::Instant};

use gpui_kit::{Context, EventEmitter, Task};
use twitty_model::{Backend, BackendError, Post, PostAction, PostId, Profile, UserRef};

use crate::{
    runtime::Runtime,
    state::{
        interactions::{Effective, Interactions, Toggle},
        long_text::Unfolded,
        motion::{self, Recent},
        toasts::Notice,
        translation::Originals,
    },
};

/// A request from a tweet row to open something; the app decides how.
#[derive(Debug, Clone, PartialEq)]
pub enum Navigate {
    /// Show the conversation around a tweet.
    Thread(PostId),
    /// Compose a reply to a tweet.
    Reply(Arc<Post>),
    /// Ask whether to retweet (or undo the retweet of) a tweet, or quote it.
    Repost(Arc<Post>),
    /// Show an account's profile.
    Profile(Arc<Profile>),
    /// Show the profile of an account mentioned in a text, which must be
    /// looked up first.
    Mention(UserRef),
    /// Add a column with an account's tweets.
    UserColumn(Arc<Profile>),
    /// Show a tweet's media in the viewer, starting at `index`.
    Media { post: Arc<Post>, index: usize },
}

/// Owns the viewer's optimistic tweet state for the whole window.
pub struct InteractionsHub {
    state: Interactions,
    backend: Arc<dyn Backend>,
    /// Toggles the viewer just turned on, whose icon "pops".
    pops: Recent<(PostId, Toggle), ()>,
    /// Translated tweets the viewer switched back to their original text.
    originals: Originals,
    /// Long tweets the viewer unfolded.
    unfolded: Unfolded,
}

impl std::fmt::Debug for InteractionsHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractionsHub").field("state", &self.state).finish_non_exhaustive()
    }
}

impl EventEmitter<Notice> for InteractionsHub {}
impl EventEmitter<Navigate> for InteractionsHub {}

impl InteractionsHub {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            state: Interactions::default(),
            backend,
            pops: Recent::default(),
            originals: Originals::default(),
            unfolded: Unfolded::default(),
        }
    }

    /// State to render for `post`.
    pub fn effective(&self, post: &Post) -> Effective {
        self.state.effective(post)
    }

    /// Whether `toggle`'s icon on tweet `id` is in its "pop", i.e. the viewer
    /// just turned it on.
    pub fn popping(&self, id: &PostId, toggle: Toggle) -> bool {
        self.pops.get(&(id.clone(), toggle), Instant::now()).is_some()
    }

    /// Whether translated tweet `id` is switched back to its original text.
    pub fn showing_original(&self, id: &PostId) -> bool {
        self.originals.showing_original(id)
    }

    /// Switches translated tweet `id` between its translation and original.
    pub fn toggle_original(&mut self, id: &PostId, cx: &mut Context<Self>) {
        self.originals.toggle(id);
        cx.notify();
    }

    /// Whether long tweet `id` is unfolded to its whole text.
    pub fn is_unfolded(&self, id: &PostId) -> bool {
        self.unfolded.is_unfolded(id)
    }

    /// Unfolds long tweet `id`, or folds it again.
    pub fn toggle_unfolded(&mut self, id: &PostId, cx: &mut Context<Self>) {
        self.unfolded.toggle(id);
        cx.notify();
    }

    /// Asks the app to open something (see [`Navigate`]).
    pub fn navigate(&mut self, to: Navigate, cx: &mut Context<Self>) {
        cx.emit(to);
    }

    /// Asks the app to show a toast.
    pub fn notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        cx.emit(notice);
    }

    /// Optimistically flips `toggle`, then confirms with the backend.
    pub fn toggle(&mut self, post: &Post, toggle: Toggle, cx: &mut Context<Self>) {
        let pending = self.state.toggle(post, toggle);
        let key = (post.id.clone(), toggle);
        if self.state.effective(post).is_on(toggle) && !cx.reduce_motion() {
            self.pops.mark(key, (), Instant::now(), motion::POP);
        } else {
            self.pops.remove(&key);
        }
        cx.notify();
        let backend = Arc::clone(&self.backend);
        let action = pending.action.clone();
        let sent = action.clone();
        let result = Runtime::global(cx).spawn(async move { backend.act(sent).await });
        cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.state.confirm(pending);
                    done(&action, cx);
                }
                Err(error) => {
                    tracing::warn!("tweet action failed, rolling back: {error}");
                    this.state.rollback(pending);
                    cx.emit(Notice::action_failed(&action, &error, chrono::Utc::now()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Remembers the state of tweets the backend just sent, so every row
    /// showing them gets their latest counters.
    pub fn record<'a>(
        &mut self,
        posts: impl IntoIterator<Item = &'a Post>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for post in posts {
            changed |= self.state.record(post);
        }
        if changed {
            cx.notify();
        }
    }

    /// Looks tweets `ids` up again and records their state. The task ends
    /// with the request's outcome.
    pub fn refresh(
        &mut self,
        ids: Vec<PostId>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), BackendError>> {
        let backend = Arc::clone(&self.backend);
        let result = Runtime::global(cx).spawn(async move { backend.posts(&ids).await });
        cx.spawn(async move |this, cx| {
            let posts = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            let posts = posts.inspect_err(|e| tracing::warn!("stats refresh failed: {e}"))?;
            this.update(cx, |this, cx| this.record(&posts, cx)).ok();
            Ok(())
        })
    }
}

/// Reports an action that went through, if it says anything.
fn done(action: &PostAction, cx: &mut Context<InteractionsHub>) {
    if let Some(notice) = Notice::action_done(action) {
        cx.emit(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};
    use twitty_model::{FeedSource, HomeMode, PageRequest, PostAction, mock::MockBackend};

    async fn first_post(backend: &MockBackend) -> Post {
        let page = backend
            .feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First)
            .await
            .unwrap();
        page.items[0].posts()[0].clone()
    }

    #[gpui_kit::test]
    async fn toggle_sends_action_and_keeps_state(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Runtime::inline()));
        let backend = MockBackend::new();
        let post = first_post(&backend).await;
        let hub = cx.new(|_| InteractionsHub::new(Arc::new(backend.clone())));

        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Like, cx));
        cx.run_until_parked();

        assert!(hub.read_with(cx, |hub, _| hub.effective(&post).viewer.liked));
        assert_eq!(backend.actions(), vec![PostAction::Like(post.id.clone())]);
    }

    #[gpui_kit::test]
    async fn turning_a_toggle_on_pops_its_icon(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Runtime::inline()));
        let backend = MockBackend::new();
        let post = first_post(&backend).await;
        assert!(!post.viewer.liked, "the mock's first tweet starts unliked");
        let hub = cx.new(|_| InteractionsHub::new(Arc::new(backend.clone())));
        let popping = |cx: &mut TestAppContext| {
            hub.read_with(cx, |hub, _| hub.popping(&post.id, Toggle::Like))
        };

        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Like, cx));
        assert!(popping(cx), "liking pops");
        assert!(!hub.read_with(cx, |hub, _| hub.popping(&post.id, Toggle::Bookmark)));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Like, cx));
        assert!(!popping(cx), "unliking doesn't");

        cx.update(|cx| cx.set_reduce_motion(true));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Like, cx));
        assert!(!popping(cx), "no pop with reduced motion");
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    async fn failed_action_rolls_back_and_reports_why(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Runtime::inline()));
        let backend = MockBackend::new();
        let post = first_post(&backend).await;
        let hub = cx.new(|_| InteractionsHub::new(Arc::new(backend.clone())));
        let notices = notices(&hub, cx);

        backend.fail_next(BackendError::Upstream("nope".into()));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Repost, cx));
        // Optimistic state is visible before the backend answers.
        assert!(hub.read_with(cx, |hub, _| hub.effective(&post).viewer.reposted));
        cx.run_until_parked();

        assert!(!hub.read_with(cx, |hub, _| hub.effective(&post).viewer.reposted));
        assert_eq!(*notices.lock().unwrap(), [Notice::error("Couldn't retweet: nope")]);
    }

    #[gpui_kit::test]
    async fn actions_that_went_through_are_reported_except_likes(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Runtime::inline()));
        let backend = MockBackend::new();
        let post = first_post(&backend).await;
        let hub = cx.new(|_| InteractionsHub::new(Arc::new(backend.clone())));
        let notices = notices(&hub, cx);

        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Like, cx));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Repost, cx));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Bookmark, cx));
        cx.run_until_parked();

        assert_eq!(
            *notices.lock().unwrap(),
            [Notice::success("Retweeted"), Notice::success("Added to Bookmarks")]
        );
    }

    /// Every notice `hub` emits, in order.
    fn notices(
        hub: &gpui_kit::Entity<InteractionsHub>,
        cx: &mut TestAppContext,
    ) -> Arc<std::sync::Mutex<Vec<Notice>>> {
        let notices = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&notices);
        cx.update(|cx| {
            cx.subscribe(hub, move |_, e: &Notice, _| seen.lock().unwrap().push(e.clone()))
                .detach();
        });
        notices
    }
}
