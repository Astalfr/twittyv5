//! The thread panel: a side panel with the conversation around one post.

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, ScrollHandle,
    Styled, Task, Window, assets::IconName, component::button::*,
    component::scroll::ScrollableElement, component::spinner::Spinner, component::*, div,
    prelude::*, px,
};
use twitty_model::{Backend, BackendError, PostId};

use crate::{
    runtime::Runtime,
    state::thread::{self, ThreadState},
    ui::{
        hub::InteractionsHub,
        post::{RowCtx, render_item},
    },
};

/// Width of the panel.
pub const PANEL_WIDTH: f32 = 600.0;

/// Emitted when the user closes the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseThread;

impl EventEmitter<CloseThread> for ThreadPanel {}

/// The conversation around `root`.
pub struct ThreadPanel {
    root: PostId,
    state: ThreadState,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    scroll: ScrollHandle,
    load: Option<Task<()>>,
}

impl std::fmt::Debug for ThreadPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadPanel").field("root", &self.root).finish_non_exhaustive()
    }
}

impl ThreadPanel {
    pub fn new(
        root: PostId,
        backend: Arc<dyn Backend>,
        hub: Entity<InteractionsHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            root,
            state: ThreadState::Loading,
            backend,
            hub,
            scroll: ScrollHandle::new(),
            load: None,
        };
        this.reload(cx);
        this
    }

    pub fn root(&self) -> &PostId {
        &self.root
    }

    pub fn state(&self) -> &ThreadState {
        &self.state
    }

    /// Loads (again) the conversation.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let root = self.root.clone();
        let result = Runtime::global(cx).spawn(async move { backend.thread(&root).await });
        self.state = ThreadState::Loading;
        self.load = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(view) => ThreadState::Loaded(Box::new(view)),
                    Err(e) => ThreadState::Failed(e),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for ThreadPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match &self.state {
            ThreadState::Loading => {
                div().p_6().flex().justify_center().child(Spinner::new()).into_any_element()
            }
            ThreadState::Failed(e) => v_flex()
                .p_6()
                .gap_3()
                .items_center()
                .text_color(theme.muted_foreground)
                .child(e.to_string())
                .child(
                    Button::new("thread-retry")
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
                .into_any_element(),
            ThreadState::Loaded(view) => {
                let rows = thread::rows(view);
                let hub = self.hub.clone();
                v_flex()
                    .children(rows.iter().enumerate().map(|(ix, item)| {
                        let rcx = RowCtx {
                            ix,
                            column_width: PANEL_WIDTH,
                            hub: hub.clone(),
                            scope: "thread".into(),
                        };
                        render_item(item, &rcx, window, cx)
                    }))
                    .when(view.more_replies.is_some(), |d| {
                        d.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("More replies on x.com"),
                        )
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .w(px(PANEL_WIDTH))
            .h_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(53.))
                    .px_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Icon::new(IconName::MessageCircle).size(px(20.)))
                    .child(
                        div().flex_1().text_lg().font_weight(FontWeight::EXTRA_BOLD).child("Post"),
                    )
                    .child(
                        Button::new("thread-close")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseThread))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("thread-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(body),
                    )
                    .vertical_scrollbar(&self.scroll),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use twitty_model::{FeedItem, FeedSource, HomeMode, PageRequest, mock::MockBackend};

    #[gpui_kit::test]
    async fn loads_the_conversation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        let page = backend
            .feed(&FeedSource::Home { mode: HomeMode::Following }, PageRequest::First)
            .await
            .unwrap();
        let post = page.items.iter().flat_map(FeedItem::posts).next().unwrap().clone();
        let shared: Arc<dyn Backend> = Arc::new(backend.clone());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let (panel, cx) = cx.add_window_view(|_, cx| {
            ThreadPanel::new(post.id.clone(), Arc::clone(&shared), hub, cx)
        });
        cx.run_until_parked();
        panel.read_with(cx, |p, _| match p.state() {
            ThreadState::Loaded(view) => assert_eq!(view.focus.id, post.id),
            other => panic!("expected a loaded thread, got {other:?}"),
        });
    }

    #[gpui_kit::test]
    fn failures_can_be_retried(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        backend.fail_next(BackendError::NotFound);
        let shared: Arc<dyn Backend> = Arc::new(backend);
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let (panel, cx) = cx.add_window_view(|_, cx| {
            ThreadPanel::new(PostId::new("missing"), Arc::clone(&shared), hub, cx)
        });
        cx.run_until_parked();
        assert!(panel.read_with(cx, |p, _| matches!(p.state(), ThreadState::Failed(_))));
    }
}
