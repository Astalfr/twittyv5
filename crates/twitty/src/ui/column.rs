//! One deck column: header, virtualised post list, paging and polling.

use std::{sync::Arc, time::Duration};

use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, ListAlignment, ListOffset,
    ListState, ParentElement, Render, SharedString, Styled, Subscription, Task, Window,
    assets::IconName, component::button::*, component::scroll::ScrollableElement,
    component::skeleton::Skeleton, component::spinner::Spinner, component::*, div, list,
    prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedSource, HomeMode};

use crate::{
    runtime::Runtime,
    state::{
        column::{ColumnModel, ListChange, LoadKind, Status},
        deck::{ColumnId, ColumnWidth},
    },
    ui::{
        hub::InteractionsHub,
        post::{RowCtx, render_item},
    },
};

/// How often a visible column checks for newer posts.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Start loading older rows when this close to the bottom.
const PREFETCH_ROWS: usize = 5;

/// Requests from a column to the deck.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnEvent {
    Remove(ColumnId),
    Move(ColumnId, isize),
    CycleWidth(ColumnId),
    SourceChanged(ColumnId, FeedSource),
}

/// A column view.
pub struct ColumnView {
    id: ColumnId,
    width: ColumnWidth,
    model: ColumnModel,
    list: ListState,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    at_top: bool,
    load_task: Option<Task<()>>,
    _poll_task: Option<Task<()>>,
    _hub_sub: Subscription,
}

impl std::fmt::Debug for ColumnView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnView")
            .field("id", &self.id)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<ColumnEvent> for ColumnView {}

impl ColumnView {
    pub fn new(
        id: ColumnId,
        source: FeedSource,
        width: ColumnWidth,
        backend: Arc<dyn Backend>,
        hub: Entity<InteractionsHub>,
        poll: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            // GPUI calls this while the ListState is mutably borrowed, so reading
            // it here (e.g. `logical_scroll_top`) would panic. React after the event.
            let visible = event.visible_range.clone();
            let weak = weak.clone();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| this.on_scroll(visible, cx)).ok();
            });
        });
        let hub_sub = cx.observe(&hub, |_, _, cx| cx.notify());
        let poll_task = poll.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL_INTERVAL).await;
                    let alive = this.update(cx, |this, cx| this.load(LoadKind::Newer, cx));
                    if alive.is_err() {
                        break;
                    }
                }
            })
        });
        let mut this = Self {
            id,
            width,
            model: ColumnModel::new(source),
            list,
            backend,
            hub,
            at_top: true,
            load_task: None,
            _poll_task: poll_task,
            _hub_sub: hub_sub,
        };
        this.load(LoadKind::Refresh, cx);
        this
    }

    pub fn id(&self) -> ColumnId {
        self.id
    }

    pub fn model(&self) -> &ColumnModel {
        &self.model
    }

    pub fn set_width(&mut self, width: ColumnWidth, cx: &mut Context<Self>) {
        self.width = width;
        // Row heights depend on the width (wrapping, media size).
        self.list.remeasure();
        cx.notify();
    }

    /// Starts a load of `kind` unless one is already running.
    pub fn load(&mut self, kind: LoadKind, cx: &mut Context<Self>) {
        let Some((kind, request)) = self.model.begin(kind, chrono::Utc::now()) else {
            return;
        };
        let backend = Arc::clone(&self.backend);
        let source = self.model.source().clone();
        let result = Runtime::global(cx).spawn(async move { backend.feed(&source, request).await });
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| this.apply(kind, result, cx)).ok();
        }));
        cx.notify();
    }

    fn apply(
        &mut self,
        kind: LoadKind,
        result: Result<twitty_model::FeedPage, BackendError>,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = &result {
            tracing::warn!(column = self.id.0, ?kind, "load failed: {e}");
        }
        match self.model.finish(kind, result) {
            ListChange::None => {}
            // +1 for the footer row.
            ListChange::Reset { count } => self.list.reset(count + 1),
            ListChange::Splice { range, count } => {
                let prepend = range.start == 0;
                self.list.splice(range, count);
                if prepend && self.at_top {
                    // Reading at the top: show the new rows right away.
                    self.list.scroll_to(ListOffset::default());
                    self.model.mark_seen();
                }
            }
        }
        cx.notify();
    }

    fn on_scroll(&mut self, visible: std::ops::Range<usize>, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        self.at_top = top.item_ix == 0 && top.offset_in_item <= px(1.);
        if visible.start < self.model.unseen() {
            // New rows above scrolled into view.
            self.model.mark_seen();
            cx.notify();
        }
        let len = self.model.items().len();
        if self.model.loaded_once() && visible.end + PREFETCH_ROWS >= len && self.model.has_older()
        {
            self.load(LoadKind::Older, cx);
        }
    }

    fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset::default());
        self.at_top = true;
        self.model.mark_seen();
        cx.notify();
    }

    fn switch_home(&mut self, mode: HomeMode, cx: &mut Context<Self>) {
        let source = FeedSource::Home { mode };
        if self.model.source() == &source {
            return;
        }
        if let ListChange::Reset { count } = self.model.set_source(source.clone()) {
            self.list.reset(count);
        }
        self.load_task = None;
        cx.emit(ColumnEvent::SourceChanged(self.id, source));
        self.load(LoadKind::Refresh, cx);
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rcx = RowCtx { ix, column_width: self.width.px(), hub: self.hub.clone() };
        match self.model.items().get(ix) {
            Some(item) => render_item(item, &rcx, window, cx),
            None => self.render_footer(cx),
        }
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let body: AnyElement = match self.model.status() {
            Status::Loading(LoadKind::Older) => Spinner::new().into_any_element(),
            Status::Failed { kind: LoadKind::Older, error } => {
                let retry = self.model.retry_kind().unwrap_or(LoadKind::Older);
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(theme.muted_foreground).child(error.to_string()))
                    .child(
                        Button::new("retry-older")
                            .small()
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| this.load(retry, cx))),
                    )
                    .into_any_element()
            }
            _ if self.model.loaded_once() && !self.model.has_older() => div()
                .text_color(theme.muted_foreground)
                .text_sm()
                .child(if self.model.items().is_empty() {
                    "Nothing to show here yet"
                } else {
                    "You're all caught up"
                })
                .into_any_element(),
            _ => div().into_any_element(),
        };
        h_flex().w_full().justify_center().py_6().child(body).into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let source = self.model.source();
        let icon = source_icon(source);
        let id = self.id;
        let header_button = |name: &'static str, icon: IconName, tip: &'static str| {
            Button::new(name).ghost().small().icon(Icon::new(icon)).tooltip(tip)
        };
        v_flex()
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(53.))
                    .px_4()
                    .gap_2()
                    .child(Icon::new(icon).size(px(20.)).text_color(theme.foreground))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_lg()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(source.title()),
                    )
                    .child(
                        header_button("refresh", IconName::RefreshCw, "Refresh").on_click(
                            cx.listener(|this, _, _, cx| this.load(LoadKind::Refresh, cx)),
                        ),
                    )
                    .child(header_button("left", IconName::ChevronLeft, "Move left").on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ColumnEvent::Move(id, -1))),
                    ))
                    .child(header_button("right", IconName::ChevronRight, "Move right").on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ColumnEvent::Move(id, 1))),
                    ))
                    .child(
                        header_button("width", IconName::MoveHorizontal, "Change width").on_click(
                            cx.listener(move |_, _, _, cx| cx.emit(ColumnEvent::CycleWidth(id))),
                        ),
                    )
                    .child(header_button("remove", IconName::X, "Remove column").on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ColumnEvent::Remove(id))),
                    )),
            )
            .when_some(home_mode(source), |d, mode| d.child(self.render_home_tabs(mode, cx)))
    }

    fn render_home_tabs(&self, mode: HomeMode, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tab = |label: &'static str, this_mode: HomeMode| {
            let active = mode == this_mode;
            v_flex()
                .id(label)
                .flex_1()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.switch_home(this_mode, cx)))
                .child(
                    div()
                        .py_3()
                        .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                        .text_color(if active { theme.foreground } else { theme.muted_foreground })
                        .child(label),
                )
                .child(
                    div()
                        .h(px(4.))
                        .w(px(56.))
                        .rounded(px(2.))
                        .when(active, |d| d.bg(theme.primary)),
                )
        };
        h_flex()
            .child(tab("For you", HomeMode::ForYou))
            .child(tab("Following", HomeMode::Following))
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if !self.model.loaded_once() {
            return match self.model.status() {
                Status::Failed { error, .. } => {
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .px_6()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .size(px(28.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(div().text_color(theme.muted_foreground).child(error.to_string()))
                        .child(Button::new("retry").primary().label("Retry").on_click(
                            cx.listener(|this, _, _, cx| this.load(LoadKind::Refresh, cx)),
                        ))
                        .into_any_element()
                }
                _ => skeleton_rows().into_any_element(),
            };
        }
        let unseen = self.model.unseen();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(list(self.list.clone(), cx.processor(Self::render_row)).size_full())
            .vertical_scrollbar(&self.list)
            .when(unseen > 0, |d| {
                d.child(
                    h_flex().absolute().top_3().w_full().justify_center().child(
                        Button::new("new-posts")
                            .primary()
                            .small()
                            .icon(Icon::new(IconName::ArrowUp))
                            .label(SharedString::from(format!(
                                "{unseen} new post{}",
                                if unseen == 1 { "" } else { "s" }
                            )))
                            .on_click(cx.listener(|this, _, _, cx| this.scroll_to_top(cx))),
                    ),
                )
            })
            .into_any_element()
    }
}

fn home_mode(source: &FeedSource) -> Option<HomeMode> {
    match source {
        FeedSource::Home { mode } => Some(*mode),
        _ => None,
    }
}

/// Header icon for a source.
pub fn source_icon(source: &FeedSource) -> IconName {
    match source {
        FeedSource::Home { .. } => IconName::House,
        FeedSource::Notifications { .. } => IconName::Bell,
        FeedSource::Search { .. } => IconName::Search,
        FeedSource::List { .. } => IconName::List,
        FeedSource::User { .. } => IconName::User,
        FeedSource::Bookmarks => IconName::Bookmark,
        FeedSource::Thread { .. } => IconName::MessageCircle,
    }
}

fn skeleton_rows() -> impl IntoElement {
    v_flex().w_full().children((0..6).map(|_| {
        h_flex()
            .items_start()
            .gap_3()
            .px_4()
            .py_3()
            .child(Skeleton::new().size(px(40.)).rounded_full())
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .child(Skeleton::new().h(px(14.)).w(px(140.)))
                    .child(Skeleton::new().h(px(14.)).w_full())
                    .child(Skeleton::new().h(px(14.)).w(px(200.))),
            )
    }))
}

impl Render for ColumnView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .flex_none()
            .w(px(self.width.px()))
            .h_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_header(cx))
            .child(self.render_body(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, TestAppContext, point};
    use twitty_model::mock::MockBackend;

    fn setup(cx: &mut TestAppContext) -> (MockBackend, Entity<ColumnView>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        let shared: Arc<dyn Backend> = Arc::new(backend.clone());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let column = cx.new(|cx| {
            ColumnView::new(
                ColumnId(1),
                FeedSource::Home { mode: HomeMode::Following },
                ColumnWidth::Medium,
                shared,
                hub,
                false,
                cx,
            )
        });
        (backend, column)
    }

    #[gpui_kit::test]
    fn loads_first_page_on_creation(cx: &mut TestAppContext) {
        let (_, column) = setup(cx);
        cx.run_until_parked();
        column.read_with(cx, |c, _| {
            assert!(c.model().loaded_once());
            assert_eq!(c.model().items().len(), MockBackend::PAGE_SIZE);
            assert_eq!(c.list.item_count(), MockBackend::PAGE_SIZE + 1, "rows + footer");
        });
    }

    #[gpui_kit::test]
    fn newer_posts_at_top_are_marked_seen(cx: &mut TestAppContext) {
        let (backend, column) = setup(cx);
        cx.run_until_parked();
        backend.push_new(&FeedSource::Home { mode: HomeMode::Following }, 3);
        column.update(cx, |c, cx| c.load(LoadKind::Newer, cx));
        cx.run_until_parked();
        column.read_with(cx, |c, _| {
            assert_eq!(c.model().items().len(), MockBackend::PAGE_SIZE + 3);
            assert_eq!(c.list.item_count(), MockBackend::PAGE_SIZE + 4);
            assert_eq!(c.model().unseen(), 0, "user was at the top");
        });
    }

    #[gpui_kit::test]
    fn newer_posts_while_scrolled_count_as_unseen(cx: &mut TestAppContext) {
        let (backend, column) = setup(cx);
        cx.run_until_parked();
        column.update(cx, |c, _| c.at_top = false);
        backend.push_new(&FeedSource::Home { mode: HomeMode::Following }, 2);
        column.update(cx, |c, cx| c.load(LoadKind::Newer, cx));
        cx.run_until_parked();
        column.read_with(cx, |c, _| assert_eq!(c.model().unseen(), 2));
        column.update(cx, |c, cx| c.scroll_to_top(cx));
        column.read_with(cx, |c, _| assert_eq!(c.model().unseen(), 0));
    }

    #[gpui_kit::test]
    fn first_load_failure_then_retry(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        backend.fail_next(BackendError::Unauthorized);
        let shared: Arc<dyn Backend> = Arc::new(backend.clone());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let column = cx.new(|cx| {
            ColumnView::new(
                ColumnId(1),
                FeedSource::Bookmarks,
                ColumnWidth::Medium,
                shared,
                hub,
                false,
                cx,
            )
        });
        cx.run_until_parked();
        column.read_with(cx, |c, _| {
            assert!(matches!(
                c.model().status(),
                Status::Failed { error: BackendError::Unauthorized, .. }
            ));
        });
        column.update(cx, |c, cx| c.load(LoadKind::Refresh, cx));
        cx.run_until_parked();
        column.read_with(cx, |c, _| assert!(c.model().loaded_once()));
    }

    #[gpui_kit::test]
    fn switching_home_mode_reloads_and_emits(cx: &mut TestAppContext) {
        let (_, column) = setup(cx);
        cx.run_until_parked();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|cx| {
            cx.subscribe(&column, move |_, e: &ColumnEvent, _| {
                seen.lock().unwrap().push(e.clone())
            })
            .detach();
        });
        column.update(cx, |c, cx| c.switch_home(HomeMode::ForYou, cx));
        cx.run_until_parked();
        column.read_with(cx, |c, _| {
            assert_eq!(c.model().source(), &FeedSource::Home { mode: HomeMode::ForYou });
            assert!(c.model().loaded_once());
        });
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [ColumnEvent::SourceChanged(ColumnId(1), FeedSource::Home { mode: HomeMode::ForYou })]
        );
    }

    /// Mounts a real window, draws it, and scrolls with wheel events until the
    /// feed is exhausted. Exercises the scroll handler, prefetch and rendering
    /// of every row shape the mock produces.
    fn scroll_to_end(cx: &mut TestAppContext, source: FeedSource) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        let shared: Arc<dyn Backend> = Arc::new(backend.clone());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let (column, cx) = cx.add_window_view(|_, cx| {
            ColumnView::new(ColumnId(1), source, ColumnWidth::Medium, shared, hub, false, cx)
        });
        cx.run_until_parked();
        assert_eq!(column.read_with(cx, |c, _| c.model().items().len()), MockBackend::PAGE_SIZE);

        let mut wheel_events = 0;
        while column.read_with(cx, |c, _| c.model().has_older()) {
            assert!(wheel_events < 500, "scrolling never reached the end of the feed");
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(150.), px(400.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-3000.))),
                ..Default::default()
            });
            cx.run_until_parked();
            wheel_events += 1;
        }

        column.read_with(cx, |c, _| {
            assert_eq!(c.model().items().len(), MockBackend::FEED_LEN);
            assert_eq!(c.list.item_count(), MockBackend::FEED_LEN + 1, "rows + footer");
            assert_eq!(c.model().status(), &Status::Idle);
            assert!(!c.at_top, "scrolled away from the top");
        });
    }

    #[gpui_kit::test]
    fn scrolling_pages_through_home(cx: &mut TestAppContext) {
        scroll_to_end(cx, FeedSource::Home { mode: HomeMode::Following });
    }

    #[gpui_kit::test]
    fn scrolling_pages_through_notifications(cx: &mut TestAppContext) {
        scroll_to_end(
            cx,
            FeedSource::Notifications { filter: twitty_model::NotificationFilter::All },
        );
    }
}
