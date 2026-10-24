//! The "Add column" dialog: pick a kind, type a query / handle / link when the
//! kind needs one, then confirm. Handles are resolved with `Backend::profile`
//! first; list columns also offer the viewer's lists (`Backend::viewer_lists`).

use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window,
    component::button::*,
    component::input::*,
    component::select::{Select, SelectEvent, SelectItem, SelectState},
    component::*,
    div,
    prelude::*,
    px,
};
use twitty_model::{Backend, BackendError, FeedSource, ListId, ListSummary, ListVisibility};

use crate::{
    runtime::Runtime,
    state::add_column::{self, ColumnKind, Parsed, ViewerLists},
    ui::button::TextButton,
    ui::column::source_icon,
    ui::emoji,
};

/// One of the viewer's lists in the picker.
#[derive(Debug, Clone)]
struct ListChoice(ListSummary);

impl SelectItem for ListChoice {
    type Value = ListId;

    fn title(&self) -> SharedString {
        self.0.name.clone().into()
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let members = match self.0.members {
            1 => "1 member".to_owned(),
            n => format!("{n} members"),
        };
        let details = match self.0.visibility {
            ListVisibility::Public => members,
            ListVisibility::Private => format!("Private · {members}"),
        };
        h_flex()
            .w_full()
            .gap_2()
            .justify_between()
            .child(div().min_w_0().truncate().child(emoji::text(self.title())))
            .child(
                div().flex_none().text_sm().text_color(cx.theme().muted_foreground).child(details),
            )
    }

    fn value(&self) -> &ListId {
        &self.0.id
    }
}

type ListPicker = SelectState<Vec<ListChoice>>;

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
    lists: ViewerLists,
    list_picker: Entity<ListPicker>,
    lists_load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for AddColumnView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddColumnView")
            .field("kind", &self.kind)
            .field("error", &self.error)
            .field("lists", &self.lists)
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
        let list_picker = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        // Picking a list replaces a typed link, which would otherwise win.
        let picked = cx.subscribe_in(
            &list_picker,
            window,
            |this, _, e: &SelectEvent<Vec<ListChoice>>, window, cx| {
                let SelectEvent::Confirm(Some(_)) = e else { return };
                this.error = None;
                this.input.update(cx, |s, cx| s.set_value("", window, cx));
                cx.notify();
            },
        );
        Self {
            kind: ColumnKind::Search,
            input,
            error: None,
            backend,
            lookup: None,
            lists: ViewerLists::default(),
            list_picker,
            lists_load: None,
            _subs: vec![sub, picked],
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

    pub fn lists(&self) -> &ViewerLists {
        &self.lists
    }

    /// The list picked in the picker.
    pub fn picked_list(&self, cx: &App) -> Option<&ListSummary> {
        self.list_picker.read(cx).selected_value().and_then(|id| self.lists.find(id))
    }

    /// Picks one of the loaded lists (tests).
    pub fn pick_list(&mut self, id: &ListId, window: &mut Window, cx: &mut Context<Self>) {
        self.list_picker.update(cx, |p, cx| p.set_selected_value(id, window, cx));
        cx.notify();
    }

    /// Loads the viewer's lists into the picker, first one picked.
    fn load_lists(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lists = ViewerLists::Loading;
        let backend = Arc::clone(&self.backend);
        let result = Runtime::global(cx).spawn(async move { backend.viewer_lists().await });
        self.lists_load = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update_in(cx, |this, window, cx| {
                this.lists_load = None;
                this.lists = match result {
                    Ok(lists) => {
                        let choices = lists.iter().cloned().map(ListChoice).collect();
                        this.list_picker.update(cx, |p, cx| {
                            p.set_items(choices, window, cx);
                            if !lists.is_empty() {
                                p.set_selected_index(Some(IndexPath::new(0)), window, cx);
                            }
                        });
                        ViewerLists::Loaded(lists)
                    }
                    Err(e) => ViewerLists::Failed(e.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Selects a kind. Nothing is added until the dialog is confirmed.
    pub fn choose(&mut self, kind: ColumnKind, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.lookup = None;
        if kind != self.kind
            && let Some(prompt) = kind.prompt()
        {
            self.input.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.set_placeholder(prompt, window, cx);
                s.focus(window, cx);
            });
        }
        self.kind = kind;
        if kind == ColumnKind::List && self.lists.needs_load() {
            self.load_lists(window, cx);
        }
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
        let parsed = match self.kind {
            ColumnKind::List => add_column::parse_list(self.picked_list(cx), &text),
            kind => add_column::parse(kind, &text),
        };
        match parsed {
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
        // Tiles styled like the sidebar entries: the icon over the kind's name.
        let kinds = ColumnKind::ALL.into_iter().map(|kind| {
            v_flex()
                .id(kind.label())
                .debug_selector(move || format!("add-kind-{}", kind.label()))
                .w(px(KIND_TILE_WIDTH))
                .h(px(KIND_TILE_HEIGHT))
                .gap_1()
                .items_center()
                .justify_center()
                .rounded_lg()
                .cursor_pointer()
                .when(kind == selected, |d| d.bg(theme.list_active).font_weight(FontWeight::BOLD))
                .hover(|s| s.bg(theme.list_hover))
                .child(Icon::new(source_icon(&sample_source(kind))).size(px(24.)))
                .child(div().text_sm().child(kind.label()))
                .on_click(cx.listener(move |this, _, window, cx| this.choose(kind, window, cx)))
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
            .when(selected == ColumnKind::List, |d| d.children(self.render_lists(cx)))
            .when(selected.prompt().is_some(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "add-column-input".into())
                        .child(Input::new(&self.input).disabled(looking_up)),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("add-cancel")
                            .ghost()
                            .text_button(Size::Medium)
                            .label("Cancel")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(AddColumnEvent::Dismissed)),
                            ),
                    )
                    .child(
                        Button::new("add-submit")
                            .primary()
                            .text_button(Size::Medium)
                            .label(if looking_up { "Looking up…" } else { "Add" })
                            .loading(looking_up)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

impl AddColumnView {
    /// The picker of the viewer's lists, or why it can't be shown.
    fn render_lists(&self, cx: &App) -> Option<AnyElement> {
        let theme = cx.theme();
        let note = |text: String, color| div().text_sm().text_color(color).child(text);
        Some(match &self.lists {
            ViewerLists::NotLoaded => return None,
            ViewerLists::Loading => {
                note("Loading your lists…".into(), theme.muted_foreground).into_any_element()
            }
            ViewerLists::Failed(e) => {
                note(format!("Couldn't load your lists: {e}"), theme.danger).into_any_element()
            }
            ViewerLists::Loaded(lists) if lists.is_empty() => {
                note("You have no lists.".into(), theme.muted_foreground).into_any_element()
            }
            ViewerLists::Loaded(_) => div()
                .debug_selector(|| "add-column-lists".into())
                .child(Select::new(&self.list_picker).placeholder("Your lists"))
                .into_any_element(),
        })
    }
}

/// Four tiles per row in the 460px dialog.
const KIND_TILE_WIDTH: f32 = 96.;
const KIND_TILE_HEIGHT: f32 = 68.;

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
    use twitty_model::{HomeMode, ListId, mock::MockBackend};

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
    fn kinds_without_input_wait_for_confirmation(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::Home, window, cx));
        assert!(events.lock().unwrap().is_empty(), "choosing alone adds nothing");
        assert_eq!(view.read_with(cx, |v, _| v.kind()), ColumnKind::Home);
        view.update_in(cx, |v, window, cx| v.submit(window, cx));
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [AddColumnEvent::Add(FeedSource::Home { mode: HomeMode::ForYou })]
        );
    }

    #[gpui_kit::test]
    fn the_input_only_shows_for_kinds_that_need_one(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, MockBackend::new());
        cx.run_until_parked();
        assert!(cx.debug_bounds("add-column-input").is_some(), "search needs a query");
        assert!(cx.debug_bounds("add-kind-Bookmarks").is_some(), "kinds are drawn as tiles");

        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::Bookmarks, window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("add-column-input").is_none());

        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::User, window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("add-column-input").is_some());
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

    fn list_column(id: &str, name: Option<&str>) -> AddColumnEvent {
        AddColumnEvent::Add(FeedSource::List { id: ListId::new(id), name: name.map(Into::into) })
    }

    #[gpui_kit::test]
    fn lists_load_into_a_picker_with_the_first_picked(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        cx.run_until_parked();
        assert!(cx.debug_bounds("add-column-lists").is_none(), "not loaded before List");

        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::List, window, cx));
        cx.run_until_parked();
        assert!(
            matches!(view.read_with(cx, |v, _| v.lists().clone()), ViewerLists::Loaded(l) if l.len() == 2)
        );
        assert!(cx.debug_bounds("add-column-lists").is_some());
        assert!(cx.debug_bounds("add-column-input").is_some(), "links still work");

        view.update_in(cx, |v, window, cx| v.submit(window, cx));
        assert_eq!(events.lock().unwrap().as_slice(), [list_column("9000", Some("Rustaceans"))]);
    }

    #[gpui_kit::test]
    fn the_picked_list_is_added_unless_a_link_is_typed(cx: &mut TestAppContext) {
        let (view, cx, events) = open(cx, MockBackend::new());
        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::List, window, cx));
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            v.pick_list(&ListId::new("9001"), window, cx);
            v.submit(window, cx);
            v.set_text("x.com/i/lists/42", window, cx);
            v.submit(window, cx);
        });
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [list_column("9001", Some("Close friends")), list_column("42", None)]
        );
    }

    #[gpui_kit::test]
    fn a_failed_list_load_says_why_and_retries(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Network("down".into()));
        let (view, cx, events) = open(cx, backend);
        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::List, window, cx));
        cx.run_until_parked();
        assert!(matches!(view.read_with(cx, |v, _| v.lists().clone()), ViewerLists::Failed(_)));
        assert!(cx.debug_bounds("add-column-lists").is_none());
        view.update_in(cx, |v, window, cx| v.submit(window, cx));
        assert!(events.lock().unwrap().is_empty(), "nothing picked, nothing typed");
        assert!(view.read_with(cx, |v, _| v.error().is_some()));

        view.update_in(cx, |v, window, cx| {
            v.choose(ColumnKind::Search, window, cx);
            v.choose(ColumnKind::List, window, cx);
        });
        cx.run_until_parked();
        assert!(matches!(view.read_with(cx, |v, _| v.lists().clone()), ViewerLists::Loaded(_)));
    }

    #[gpui_kit::test]
    fn without_lists_only_a_link_can_be_added(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        backend.set_lists(Vec::new());
        let (view, cx, events) = open(cx, backend);
        view.update_in(cx, |v, window, cx| v.choose(ColumnKind::List, window, cx));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.lists().clone()), ViewerLists::Loaded(Vec::new()));
        assert!(cx.debug_bounds("add-column-lists").is_none());
        view.update_in(cx, |v, window, cx| v.submit(window, cx));
        assert_eq!(
            view.read_with(cx, |v, _| v.error().map(str::to_owned)).as_deref(),
            Some("Pick one of your lists or paste a list link.")
        );
        view.update_in(cx, |v, window, cx| {
            v.set_text("42", window, cx);
            v.submit(window, cx);
        });
        assert_eq!(events.lock().unwrap().as_slice(), [list_column("42", None)]);
    }
}
