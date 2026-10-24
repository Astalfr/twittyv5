//! The "Search people" dialog: type a name or @handle, the results follow
//! once typing pauses (`state::user_search::DEBOUNCE`); pick an account to
//! open its profile.

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, component::button::*, component::input::*,
    component::spinner::Spinner, component::*, div, prelude::*, px,
};
use twitty_model::{Backend, BackendError, Profile};

use crate::{
    format::compact_count,
    runtime::Runtime,
    state::user_search::{self, UserSearch},
    ui::button::TextButton,
    ui::post::{avatar, verified_badge},
};

/// Diameter of a result's avatar.
const AVATAR: f32 = 40.0;
/// Results scroll past this height.
const RESULTS_MAX_H: f32 = 420.0;

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum UserSearchEvent {
    /// The user picked this account.
    Open(Arc<Profile>),
    Dismissed,
}

impl EventEmitter<UserSearchEvent> for UserSearchView {}

/// The "Search people" dialog's view.
pub struct UserSearchView {
    input: Entity<InputState>,
    search: UserSearch,
    backend: Arc<dyn Backend>,
    /// Demo data: only the sample accounts can be found.
    demo: bool,
    /// Waits for typing to pause; each change replaces (cancels) it.
    debounce: Option<Task<()>>,
    /// The request in flight; replacing it cancels the previous one.
    request: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for UserSearchView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserSearchView").field("search", &self.search).finish_non_exhaustive()
    }
}

impl UserSearchView {
    /// The dialog on `backend`; `demo` says it only knows sample accounts.
    pub fn new(
        backend: Arc<dyn Backend>,
        demo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Name or @handle"));
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe(&input, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.input_changed(cx);
            }
        });
        Self {
            input,
            search: UserSearch::default(),
            backend,
            demo,
            debounce: None,
            request: None,
            _subs: vec![sub],
        }
    }

    pub fn search(&self) -> &UserSearch {
        &self.search
    }

    /// Searches once the input stayed unchanged for the debounce delay. An
    /// emptied input clears the results at once.
    fn input_changed(&mut self, cx: &mut Context<Self>) {
        if user_search::query(&self.input.read(cx).value()).is_none() {
            self.debounce = None;
            self.run(cx);
            return;
        }
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(user_search::DEBOUNCE).await;
            this.update(cx, |this, cx| this.run(cx)).ok();
        }));
    }

    /// Searches for what the input holds now.
    fn run(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value();
        let Some((generation, query)) = self.search.start(&text) else {
            if !self.search.is_searching() {
                self.request = None;
            }
            cx.notify();
            return;
        };
        let backend = Arc::clone(&self.backend);
        let result = Runtime::global(cx).spawn(async move { backend.search_users(&query).await });
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            match &result {
                Ok(users) => tracing::debug!("people search found {} accounts", users.len()),
                Err(e) => tracing::warn!("people search failed: {e}"),
            }
            this.update(cx, |this, cx| {
                this.search.finish(generation, result);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Picks result `ix`.
    pub fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(profile) = self.search.results().get(ix) {
            cx.emit(UserSearchEvent::Open(Arc::clone(profile)));
        }
    }
}

impl Render for UserSearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.search.results().iter().enumerate().map(|(ix, profile)| {
            let name = h_flex()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(profile.display_name.clone()),
                )
                .when_some(profile.badges.verified, |d, kind| {
                    d.child(verified_badge(kind, &theme))
                });
            let handle = format!(
                "@{} · {} followers",
                profile.handle,
                compact_count(profile.counts.followers)
            );
            h_flex()
                .id(("user-result", ix))
                .debug_selector(move || format!("user-result-{ix}"))
                .w_full()
                .gap_3()
                .px_2()
                .py_2()
                .rounded_lg()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.open(ix, cx)))
                .child(avatar(profile, AVATAR).flex_none())
                .child(v_flex().min_w_0().child(name).child(
                    div().text_xs().text_color(theme.muted_foreground).truncate().child(handle),
                ))
        });
        let has_results = !self.search.results().is_empty();
        v_flex()
            .w(px(460.))
            .gap_4()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Search people"))
            .child(
                Input::new(&self.input)
                    .when(self.search.is_searching(), |i| i.suffix(Spinner::new().small())),
            )
            .when(self.demo, |d| {
                d.child(div().text_xs().text_color(theme.muted_foreground).child(
                    "Demo mode: only the sample accounts can be found. Sign in to search Twitter.",
                ))
            })
            .when_some(self.search.message(), |d, message| {
                let color =
                    if self.search.failed() { theme.danger } else { theme.muted_foreground };
                d.child(div().text_sm().text_color(color).child(message))
            })
            .when(has_results, |d| {
                d.child(
                    v_flex()
                        .id("user-results")
                        .max_h(px(RESULTS_MAX_H))
                        .overflow_y_scroll()
                        .children(rows),
                )
            })
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("user-search-cancel")
                        .ghost()
                        .text_button(Size::Medium)
                        .label("Cancel")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(UserSearchEvent::Dismissed))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;
    use twitty_model::mock::MockBackend;

    type Events = Arc<std::sync::Mutex<Vec<UserSearchEvent>>>;

    fn open(
        cx: &mut TestAppContext,
        backend: MockBackend,
    ) -> (Entity<UserSearchView>, &mut VisualTestContext, Events) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            UserSearchView::new(Arc::new(backend), false, window, cx)
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &UserSearchEvent, _| {
                seen.lock().unwrap().push(e.clone())
            })
            .detach();
        });
        // A frame, so the focused input takes typing.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        (view, cx, events)
    }

    fn wait(cx: &mut VisualTestContext, time: Duration) {
        cx.executor().advance_clock(time);
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn handles(view: &Entity<UserSearchView>, cx: &mut VisualTestContext) -> Vec<String> {
        view.read_with(cx, |v, _| {
            v.search().results().iter().map(|p| p.handle.to_string()).collect()
        })
    }

    #[gpui_kit::test]
    fn results_follow_typing_once_it_pauses(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, MockBackend::new());
        cx.simulate_input("@gra");
        wait(cx, Duration::from_millis(200));
        cx.simulate_input("ce");
        wait(cx, Duration::from_millis(200));
        assert!(handles(&view, cx).is_empty(), "typing restarted the delay");
        assert!(!view.read_with(cx, |v, _| v.search().is_searching()), "nothing sent yet");
        wait(cx, Duration::from_millis(100));
        assert_eq!(handles(&view, cx), ["grace"]);
    }

    #[gpui_kit::test]
    fn emptying_the_input_clears_the_results_at_once(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, MockBackend::new());
        cx.simulate_input("ada");
        wait(cx, user_search::DEBOUNCE);
        assert_eq!(handles(&view, cx), ["ada"]);
        cx.simulate_keystrokes("backspace backspace backspace");
        cx.run_until_parked();
        assert!(handles(&view, cx).is_empty());
        assert_eq!(view.read_with(cx, |v, _| v.search().message()), None);
    }

    #[gpui_kit::test]
    fn clicking_a_result_opens_that_account(cx: &mut TestAppContext) {
        let (_, cx, events) = open(cx, MockBackend::new());
        cx.simulate_input("ada");
        wait(cx, user_search::DEBOUNCE);
        let row = cx.debug_bounds("user-result-0").expect("a result row is drawn");
        cx.simulate_click(row.center(), gpui_kit::Modifiers::default());
        let events = events.lock().unwrap();
        let [UserSearchEvent::Open(profile)] = events.as_slice() else {
            panic!("expected the account to open, got {events:?}");
        };
        assert_eq!(profile.handle, "ada");
    }

    #[gpui_kit::test]
    fn a_failed_search_shows_why(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Network("down".into()));
        let (view, cx, events) = open(cx, backend);
        cx.simulate_input("ada");
        wait(cx, user_search::DEBOUNCE);
        assert!(events.lock().unwrap().is_empty());
        assert!(view.read_with(cx, |v, _| v.search().failed()));
        assert!(handles(&view, cx).is_empty());
    }
}
