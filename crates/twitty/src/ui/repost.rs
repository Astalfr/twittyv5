//! The retweet dialog, from a tweet's retweet button or `t`: a preview of the
//! tweet, then **Retweet** (or **Undo retweet**), **Quote** or **Cancel**.
//! Enter confirms and `q` quotes.

use std::sync::Arc;

use gpui_kit::{
    Context, EventEmitter, FocusHandle, FontWeight, IntoElement, ParentElement, Render, Styled,
    Window, assets::IconName, component::button::*, component::*, div, prelude::*, px,
};
use twitty_model::Post;

use crate::{
    state::repost::RepostChoice,
    ui::{button::TextButton, keys, post::post_preview},
};

/// Width of the dialog card.
const WIDTH: f32 = 480.;
/// Its padding on each side (`p_6`).
const PADDING: f32 = 24.;

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum RepostEvent {
    /// Retweet, or undo the retweet, as the dialog offered.
    Confirm(RepostChoice),
    /// Write a quote of the tweet instead.
    Quote,
    Dismissed,
}

impl EventEmitter<RepostEvent> for RepostView {}

/// The retweet dialog's view.
pub struct RepostView {
    post: Arc<Post>,
    choice: RepostChoice,
    /// Whether the viewer switched this translated tweet to its original.
    showing_original: bool,
    focus: FocusHandle,
}

impl std::fmt::Debug for RepostView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepostView")
            .field("post", &self.post.id)
            .field("choice", &self.choice)
            .finish_non_exhaustive()
    }
}

impl RepostView {
    /// The dialog for `post`, offering `choice`. It takes the keyboard, so
    /// the deck's shortcuts don't act behind it.
    pub fn new(
        post: Arc<Post>,
        choice: RepostChoice,
        showing_original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { post, choice, showing_original, focus }
    }

    pub fn post(&self) -> &Arc<Post> {
        &self.post
    }

    pub fn choice(&self) -> RepostChoice {
        self.choice
    }

    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        cx.emit(RepostEvent::Confirm(self.choice));
    }

    pub fn quote(&mut self, cx: &mut Context<Self>) {
        cx.emit(RepostEvent::Quote);
    }
}

impl Render for RepostView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let choice = self.choice;
        v_flex()
            .track_focus(&self.focus)
            .key_context(keys::REPOST_DIALOG)
            .on_action(cx.listener(|this, _: &keys::ConfirmRepost, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &keys::QuoteInstead, _, cx| this.quote(cx)))
            .w(px(WIDTH))
            .gap_4()
            .p(px(PADDING))
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child(choice.title()))
            .child(post_preview(
                "repost-preview",
                &self.post,
                self.showing_original,
                WIDTH - 2. * PADDING,
                (8, 220.),
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("repost-quote")
                            .outline()
                            .icon(IconName::Pencil)
                            .text_button(Size::Medium)
                            .label("Quote")
                            .on_click(cx.listener(|this, _, _, cx| this.quote(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("repost-cancel")
                            .ghost()
                            .text_button(Size::Medium)
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(RepostEvent::Dismissed))),
                    )
                    .child(
                        Button::new("repost-confirm")
                            .map(|b| match choice {
                                RepostChoice::Repost => b.primary(),
                                RepostChoice::Undo => b.danger(),
                            })
                            .icon(IconName::Repeat2)
                            .text_button(Size::Medium)
                            .label(choice.label())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use twitty_model::{Backend, FeedSource, HomeMode, PageRequest, mock::MockBackend};

    fn open(
        cx: &mut TestAppContext,
        choice: RepostChoice,
    ) -> (Entity<RepostView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<RepostEvent>>>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            keys::bind(cx);
        });
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                backend.feed(&FeedSource::Home { mode: HomeMode::ForYou }, PageRequest::First),
            )
            .unwrap();
        let post = Arc::new(page.items[0].posts()[0].clone());
        let (view, cx) =
            cx.add_window_view(|window, cx| RepostView::new(post, choice, false, window, cx));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &RepostEvent, _| seen.lock().unwrap().push(e.clone()))
                .detach();
        });
        cx.run_until_parked();
        (view, cx, events)
    }

    #[gpui_kit::test]
    fn shows_the_post_and_enter_confirms(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, RepostChoice::Repost);
        let body = cx.debug_bounds("repost-preview-body").expect("the tweet's text is shown");
        assert!(body.size.height > px(0.));
        let text = view.read_with(cx, |v, _| v.post().body.text().to_owned());
        assert!(!text.is_empty());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*events.lock().unwrap(), [RepostEvent::Confirm(RepostChoice::Repost)]);
    }

    #[gpui_kit::test]
    fn q_quotes_and_undo_is_offered(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, RepostChoice::Undo);
        assert_eq!(view.read_with(cx, |v, _| v.choice()), RepostChoice::Undo);
        cx.simulate_keystrokes("q");
        cx.run_until_parked();
        assert_eq!(*events.lock().unwrap(), [RepostEvent::Quote]);
    }
}
