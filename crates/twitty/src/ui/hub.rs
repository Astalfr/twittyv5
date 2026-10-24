//! Shared, observable wrapper around [`Interactions`] that also sends the
//! actions to the backend and rolls back on failure.

use std::{sync::Arc, time::Instant};

use gpui_kit::{Context, EventEmitter};
use twitty_model::{Backend, BackendError, Post, PostId, Profile, UserRef};

use crate::{
    runtime::Runtime,
    state::{
        interactions::{Effective, Interactions, Toggle},
        motion::{self, Recent},
    },
};

/// Emitted when an optimistic action had to be undone.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionFailed(pub BackendError);

/// A request from a post row to open something; the app decides how.
#[derive(Debug, Clone, PartialEq)]
pub enum Navigate {
    /// Show the conversation around a post.
    Thread(PostId),
    /// Compose a reply to a post.
    Reply(Arc<Post>),
    /// Show an account's profile.
    Profile(Arc<Profile>),
    /// Show the profile of an account mentioned in a text, which must be
    /// looked up first.
    Mention(UserRef),
    /// Add a column with an account's posts.
    UserColumn(Arc<Profile>),
    /// Show a post's media in the viewer, starting at `index`.
    Media { post: Arc<Post>, index: usize },
}

/// Owns the viewer's optimistic post state for the whole window.
pub struct InteractionsHub {
    state: Interactions,
    backend: Arc<dyn Backend>,
    /// Toggles the viewer just turned on, whose icon "pops".
    pops: Recent<(PostId, Toggle), ()>,
}

impl std::fmt::Debug for InteractionsHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractionsHub").field("state", &self.state).finish_non_exhaustive()
    }
}

impl EventEmitter<ActionFailed> for InteractionsHub {}
impl EventEmitter<Navigate> for InteractionsHub {}

impl InteractionsHub {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self { state: Interactions::default(), backend, pops: Recent::default() }
    }

    /// State to render for `post`.
    pub fn effective(&self, post: &Post) -> Effective {
        self.state.effective(post)
    }

    /// Whether `toggle`'s icon on post `id` is in its "pop", i.e. the viewer
    /// just turned it on.
    pub fn popping(&self, id: &PostId, toggle: Toggle) -> bool {
        self.pops.get(&(id.clone(), toggle), Instant::now()).is_some()
    }

    /// Asks the app to open something (see [`Navigate`]).
    pub fn navigate(&mut self, to: Navigate, cx: &mut Context<Self>) {
        cx.emit(to);
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
        let result = Runtime::global(cx).spawn(async move { backend.act(action).await });
        cx.spawn(async move |this, cx| {
            let error = match result.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e,
                Err(e) => BackendError::Network(e.to_string()),
            };
            tracing::warn!("post action failed, rolling back: {error}");
            this.update(cx, |this, cx| {
                this.state.rollback(pending);
                cx.emit(ActionFailed(error));
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        assert!(!post.viewer.liked, "the mock's first post starts unliked");
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
    async fn failed_action_rolls_back_and_emits(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Runtime::inline()));
        let backend = MockBackend::new();
        let post = first_post(&backend).await;
        let hub = cx.new(|_| InteractionsHub::new(Arc::new(backend.clone())));
        let failures = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&failures);
        cx.update(|cx| {
            cx.subscribe(&hub, move |_, e: &ActionFailed, _| seen.lock().unwrap().push(e.clone()))
                .detach();
        });

        backend.fail_next(BackendError::Upstream("nope".into()));
        hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Repost, cx));
        // Optimistic state is visible before the backend answers.
        assert!(hub.read_with(cx, |hub, _| hub.effective(&post).viewer.reposted));
        cx.run_until_parked();

        assert!(!hub.read_with(cx, |hub, _| hub.effective(&post).viewer.reposted));
        assert_eq!(failures.lock().unwrap().len(), 1);
    }
}
