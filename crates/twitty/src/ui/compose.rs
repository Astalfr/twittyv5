//! The compose dialog: write a post or a reply, with X's length counter.

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, component::button::*, component::input::*, component::*, div,
    prelude::*, px,
};
use twitty_model::{Backend, BackendError, Post};

use crate::{
    runtime::Runtime,
    state::compose::{self, MAX_LENGTH},
};

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum ComposeEvent {
    /// The post was published.
    Published(Arc<Post>),
    Dismissed,
}

impl EventEmitter<ComposeEvent> for ComposeView {}

/// The compose dialog's view.
pub struct ComposeView {
    reply_to: Option<Arc<Post>>,
    text: Entity<TextareaState>,
    backend: Arc<dyn Backend>,
    error: Option<String>,
    sending: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for ComposeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeView")
            .field("reply_to", &self.reply_to.as_ref().map(|p| &p.id))
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl ComposeView {
    pub fn new(
        backend: Arc<dyn Backend>,
        reply_to: Option<Arc<Post>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = if reply_to.is_some() { "Post your reply" } else { "What's happening?" };
        let text =
            cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 12).placeholder(placeholder));
        // Re-render on every edit so the counter follows the text.
        let sub = cx.subscribe(&text, |_, _, _: &InputEvent, cx| cx.notify());
        text.update(cx, |s, cx| s.focus(window, cx));
        Self { reply_to, text, backend, error: None, sending: None, _subs: vec![sub] }
    }

    pub fn reply_to(&self) -> Option<&Arc<Post>> {
        self.reply_to.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Types `text` at the cursor (tests).
    pub fn type_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.text.update(cx, |s, cx| s.insert(text, window, cx));
        cx.notify();
    }

    /// Publishes the text if it fits.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value();
        if self.sending.is_some() || !compose::can_post(&text) {
            return;
        }
        let draft = compose::draft(&text, self.reply_to.as_deref());
        let backend = Arc::clone(&self.backend);
        let result = Runtime::global(cx).spawn(async move { backend.publish(draft).await });
        self.error = None;
        self.sending = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| {
                this.sending = None;
                match result {
                    Ok(post) => cx.emit(ComposeEvent::Published(Arc::new(post))),
                    Err(e) => {
                        tracing::warn!("publishing failed: {e}");
                        this.error = Some(format!("Couldn't post: {e}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for ComposeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let text = self.text.read(cx).value();
        let remaining = compose::remaining(&text);
        let can_post = compose::can_post(&text);
        let sending = self.sending.is_some();
        let counter_color = match remaining {
            r if r < 0 => theme.danger,
            r if r < 20 => theme.warning,
            _ => theme.muted_foreground,
        };
        v_flex()
            .w(px(560.))
            .gap_3()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(
                div().text_xl().font_weight(FontWeight::BOLD).child(if self.reply_to.is_some() {
                    "Reply"
                } else {
                    "New post"
                }),
            )
            .when_some(self.reply_to.clone(), |d, parent| {
                d.child(
                    v_flex()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded(px(12.))
                        .bg(theme.muted)
                        .text_sm()
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(format!("Replying to @{}", parent.author.handle)),
                        )
                        .child(div().line_clamp(3).child(parent.body.text().to_owned())),
                )
            })
            .child(Textarea::new(&self.text).disabled(sending))
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(counter_color)
                            .child(format!("{remaining} / {MAX_LENGTH}")),
                    )
                    .child(
                        Button::new("compose-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposeEvent::Dismissed))),
                    )
                    .child(
                        Button::new("compose-post")
                            .primary()
                            .label(if self.reply_to.is_some() { "Reply" } else { "Post" })
                            .loading(sending)
                            .disabled(!can_post || sending)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use twitty_model::mock::MockBackend;

    fn open(
        cx: &mut TestAppContext,
        backend: MockBackend,
        reply_to: Option<Arc<Post>>,
    ) -> (Entity<ComposeView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<ComposeEvent>>>)
    {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            ComposeView::new(Arc::new(backend), reply_to, window, cx)
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &ComposeEvent, _| seen.lock().unwrap().push(e.clone()))
                .detach();
        });
        (view, cx, events)
    }

    #[gpui_kit::test]
    fn posts_the_text(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new(), None);
        view.update_in(cx, |v, window, cx| v.type_text("hello from Twitty", window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();
        let events = events.lock().unwrap();
        let [ComposeEvent::Published(post)] = events.as_slice() else { panic!("{events:?}") };
        assert_eq!(post.body.text(), "hello from Twitty");
    }

    #[gpui_kit::test]
    fn replies_carry_the_parent(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        let parent =
            Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .block_on(backend.publish(twitty_model::Draft {
                        text: "parent".into(),
                        ..Default::default()
                    }))
                    .unwrap(),
            );
        let (view, cx, events) = open(cx, backend, Some(Arc::clone(&parent)));
        view.update_in(cx, |v, window, cx| v.type_text("a reply", window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();
        let events = events.lock().unwrap();
        let [ComposeEvent::Published(post)] = events.as_slice() else { panic!("{events:?}") };
        assert_eq!(post.reply_to.as_ref().and_then(|r| r.post.as_ref()), Some(&parent.id));
    }

    #[gpui_kit::test]
    fn too_long_or_empty_text_is_not_sent(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new(), None);
        view.update(cx, |v, cx| v.submit(cx));
        view.update_in(cx, |v, window, cx| v.type_text(&"a".repeat(281), window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();
        assert!(events.lock().unwrap().is_empty());
    }

    #[gpui_kit::test]
    fn failures_keep_the_text_and_explain(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Upstream("X error 187: Status is a duplicate.".into()));
        let (view, cx, events) = open(cx, backend, None);
        view.update_in(cx, |v, window, cx| v.type_text("again", window, cx));
        view.update(cx, |v, cx| v.submit(cx));
        cx.run_until_parked();
        assert!(events.lock().unwrap().is_empty());
        let error = view.read_with(cx, |v, _| v.error().map(str::to_owned)).unwrap();
        assert!(error.contains("duplicate"));
        assert_eq!(view.read_with(cx, |v, cx| v.text.read(cx).value().to_string()), "again");
    }
}
