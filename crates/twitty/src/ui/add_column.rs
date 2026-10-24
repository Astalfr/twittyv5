//! The "Add column" dialog: pick a kind, type a query / handle / link when the
//! kind needs one. Handles are resolved with `Backend::profile` first.

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, component::button::*, component::input::*, component::*, div,
    prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedSource};

use crate::{
    runtime::Runtime,
    state::add_column::{self, ColumnKind, Parsed},
    ui::column::source_icon,
};

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum AddColumnEvent {
    Add(FeedSource),
    Dismissed,
}

impl EventEmitter<AddColumnEvent> for AddColumnView {}

/// The "Add column" dialog's view.
pub struct AddColumnView {
    kind: ColumnKind,
    input: Entity<InputState>,
    error: Option<String>,
    backend: Arc<dyn Backend>,
    lookup: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for AddColumnView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddColumnView")
            .field("kind", &self.kind)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl AddColumnView {
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        });
        Self {
            kind: ColumnKind::Search,
            input,
            error: None,
            backend,
            lookup: None,
            _subs: vec![sub],
        }
        .with_prompt(window, cx)
    }

    fn with_prompt(self, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = self.kind.prompt().unwrap_or_default();
        self.input.update(cx, |s, cx| {
            s.set_placeholder(prompt, window, cx);
            s.focus(window, cx);
        });
        self
    }

    pub fn kind(&self) -> ColumnKind {
        self.kind
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_looking_up(&self) -> bool {
        self.lookup.is_some()
    }

    /// Selects a kind; kinds without input are added right away.
    pub fn choose(&mut self, kind: ColumnKind, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = kind;
        self.error = None;
        self.lookup = None;
        if kind.prompt().is_none() {
            self.submit(window, cx);
            return;
        }
        let prompt = kind.prompt().unwrap_or_default();
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_placeholder(prompt, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Sets the input text (tests).
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// Reads the input and adds the column, looking the handle up first for users.
    pub fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.lookup.is_some() {
            return;
        }
        let text = self.input.read(cx).value();
        match add_column::parse(self.kind, &text) {
            Err(e) => self.error = Some(e.to_string()),
            Ok(Parsed::Source(source)) => cx.emit(AddColumnEvent::Add(source)),
            Ok(Parsed::LookUpUser(handle)) => {
                self.error = None;
                let backend = Arc::clone(&self.backend);
                let wanted = handle.clone();
                let result =
                    Runtime::global(cx).spawn(async move { backend.profile(&wanted).await });
                self.lookup = Some(cx.spawn(async move |this, cx| {
                    let result = match result.await {
                        Ok(r) => r,
                        Err(e) => Err(BackendError::Network(e.to_string())),
                    };
                    this.update(cx, |this, cx| {
                        this.lookup = None;
                        match result {
                            Ok(profile) => cx.emit(AddColumnEvent::Add(add_column::user_source(
                                profile.id.clone(),
                                &profile.handle,
                            ))),
                            Err(BackendError::NotFound) => {
                                this.error = Some(format!("No account called @{handle}."));
                            }
                            Err(e) => this.error = Some(format!("Couldn't look up @{handle}: {e}")),
                        }
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }
}

impl Render for AddColumnView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.kind;
        let kinds = ColumnKind::ALL.into_iter().map(|kind| {
            let icon = source_icon(&sample_source(kind));
            let button = Button::new(kind.label())
                .icon(Icon::new(icon))
                .label(kind.label())
                .small()
                .on_click(cx.listener(move |this, _, window, cx| this.choose(kind, window, cx)));
            if kind == selected && kind.prompt().is_some() {
                button.primary()
            } else {
                button.ghost()
            }
        });
        let looking_up = self.is_looking_up();
        v_flex()
            .w(px(460.))
            .gap_4()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Add column"))
            .child(h_flex().flex_wrap().gap_2().children(kinds))
            .when(selected.prompt().is_some(), |d| {
                d.child(Input::new(&self.input).disabled(looking_up))
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("add-cancel").ghost().label("Cancel").on_click(
                            cx.listener(|_, _, _, cx| cx.emit(AddColumnEvent::Dismissed)),
                        ),
                    )
                    .when(selected.prompt().is_some(), |d| {
                        d.child(
                            Button::new("add-submit")
                                .primary()
                                .label(if looking_up { "Looking up…" } else { "Add" })
                                .loading(looking_up)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.submit(window, cx)),
                                ),
                        )
                    }),
            )
    }
}

/// A representative source of `kind`, to pick its icon.
fn sample_source(kind: ColumnKind) -> FeedSource {
    match add_column::parse(kind, "1") {
        Ok(Parsed::Source(source)) => source,
        _ => add_column::user_source(twitty_model::UserId::new("1"), "1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use twitty_model::{HomeMode, mock::MockBackend};

    fn open(
        cx: &mut TestAppContext,
        backend: MockBackend,
    ) -> (Entity<AddColumnView>, &mut VisualTestContext, Arc<std::sync::Mutex<Vec<AddColumnEvent>>>)
    {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let (view, cx) =
            cx.add_window_view(|window, cx| AddColumnView::new(Arc::new(backend), window, cx));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &AddColumnEvent, _| {
                seen.lock().unwrap().push(e.clone())
            })
            .detach();
        });
        (view, cx, events)
    }

    #[gpui_kit::test]
    fn kinds_without_input_are_added_at_once(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::Home, window, cx));
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [AddColumnEvent::Add(FeedSource::Home { mode: HomeMode::ForYou })]
        );
    }

    #[gpui_kit::test]
    fn search_adds_the_typed_query(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        view.update_in(cx, |v, window, cx| {
            v.set_text("gpui", window, cx);
            v.submit(window, cx);
        });
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [AddColumnEvent::Add(FeedSource::Search { query, .. })] if query == "gpui"
        ));
    }

    #[gpui_kit::test]
    fn users_are_looked_up_before_adding(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        let (view, cx, events) = open(cx, backend.clone());
        view.update_in(cx, |v, window, cx| {
            v.choose(ColumnKind::User, window, cx);
            v.set_text("https://x.com/ada", window, cx);
            v.submit(window, cx);
        });
        cx.run_until_parked();
        let events = events.lock().unwrap();
        let [AddColumnEvent::Add(FeedSource::User { handle, .. })] = events.as_slice() else {
            panic!("expected a user column, got {events:?}");
        };
        assert_eq!(handle, "ada");
    }

    #[gpui_kit::test]
    fn unknown_users_show_an_error(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::NotFound);
        let (view, cx, events) = open(cx, backend);
        view.update_in(cx, |v, window, cx| {
            v.choose(ColumnKind::User, window, cx);
            v.set_text("@nobody", window, cx);
            v.submit(window, cx);
        });
        cx.run_until_parked();
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(
            view.read_with(cx, |v, _| v.error().map(str::to_owned)).as_deref(),
            Some("No account called @nobody.")
        );
    }

    #[gpui_kit::test]
    fn bad_links_are_explained(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        view.update_in(cx, |v, window, cx| {
            v.choose(ColumnKind::Post, window, cx);
            v.set_text("https://x.com/NASA", window, cx);
            v.submit(window, cx);
        });
        assert!(events.lock().unwrap().is_empty());
        assert!(view.read_with(cx, |v, _| v.error().is_some()));
    }
}
