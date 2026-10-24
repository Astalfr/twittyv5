//! Shared, observable wrapper around [`Interactions`] that also sends the
//! actions to the backend and rolls back on failure.

use std::sync::Arc;

use gpui_kit::{Context, EventEmitter};
use twitty_model::{Backend, BackendError, Post, PostId, Profile};

use crate::{
    runtime::Runtime,
    state::interactions::{Effective, Interactions, Toggle},
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
    /// Add a column with an account's posts.
    Profile(Arc<Profile>),
    /// Show a post's media in the viewer, starting at `index`.
    Media { post: Arc<Post>, index: usize },
}

/// Owns the viewer's optimistic post state for the whole window.
pub struct InteractionsHub {
    state: Interactions,
    backend: Arc<dyn Backend>,
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
        Self { state: Interactions::default(), backend }
    }

    /// State to render for `post`.
    pub fn effective(&self, post: &Post) -> Effective {
        self.state.effective(post)
    }

    /// Asks the app to open something (see [`Navigate`]).
    pub fn navigate(&mut self, to: Navigate, cx: &mut Context<Self>) {
        cx.emit(to);
    }

    /// Optimistically flips `toggle`, then confirms with the backend.
    pub fn toggle(&mut self, post: &Post, toggle: Toggle, cx: &mut Context<Self>) {
        let pending = self.state.toggle(post, toggle);
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
