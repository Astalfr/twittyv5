//! One deck column: header, virtualised post list, paging and polling.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{
    Animation, AnimationExt, AnyElement, Context, ElementId, Entity, EventEmitter, FontWeight,
    IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Render, SharedString, Styled,
    Subscription, SystemNotification, Task, Window, assets::IconName, base, component::button::*,
    component::scroll::ScrollableElement, component::skeleton::Skeleton,
    component::spinner::Spinner, component::*, div, list, prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedSource};

use crate::{
    runtime::Runtime,
    sound,
    state::{
        alerts,
        column::{ColumnModel, ListChange, LoadKind, Status},
        deck::{ColumnConfig, ColumnId, ColumnSettings},
        motion::{self, Entrance, Recent},
        poll,
        sources::{self, Tab},
    },
    ui::{
        hub::InteractionsHub,
        post::{RowCtx, render_item},
    },
};

/// Start loading older rows when this close to the bottom.
const PREFETCH_ROWS: usize = 5;
/// Distance of the "new posts" pill from the top of the list.
const PILL_TOP: f32 = 12.0;

/// Requests from a column to the deck.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnEvent {
    Remove(ColumnId),
    Move(ColumnId, isize),
    /// The user asked for the column's settings dialog.
    OpenSettings(ColumnId),
    SourceChanged(ColumnId, FeedSource),
}

/// A column view.
pub struct ColumnView {
    id: ColumnId,
    settings: ColumnSettings,
    model: ColumnModel,
    /// Whether the pointer is over the header, which shows its buttons.
    header_hovered: bool,
    list: ListState,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    at_top: bool,
    /// Rows that just arrived and animate in (by item key).
    arrivals: Recent<String, Entrance>,
    /// Whether keyboard shortcuts act on this column.
    active: bool,
    load_task: Option<Task<()>>,
    /// How often to check for newer posts; `None` pauses polling. Set by the
    /// app from the provider's rate limits (`state::poll`).
    poll_interval: Option<Duration>,
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
        config: &ColumnConfig,
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
                    let Ok(every) = this.read_with(cx, |this, _| this.poll_interval) else {
                        break;
                    };
                    cx.background_executor().timer(every.unwrap_or(poll::MIN_INTERVAL)).await;
                    if every.is_none() {
                        continue;
                    }
                    let alive = this.update(cx, |this, cx| this.load(LoadKind::Newer, cx));
                    if alive.is_err() {
                        break;
                    }
                }
            })
        });
        let mut this = Self {
            id: config.id,
            settings: config.settings.clone(),
            model: ColumnModel::new(config.source.clone()),
            header_hovered: false,
            list,
            backend,
            hub,
            at_top: true,
            arrivals: Recent::default(),
            active: false,
            load_task: None,
            poll_interval: poll.then_some(poll::MIN_INTERVAL),
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

    pub fn poll_interval(&self) -> Option<Duration> {
        self.poll_interval
    }

    /// Changes how often the column polls (`None` pauses it).
    pub fn set_poll_interval(&mut self, every: Option<Duration>) {
        self.poll_interval = every;
    }

    pub fn settings(&self) -> &ColumnSettings {
        &self.settings
    }

    /// The header's title: the column's name, else its source's title.
    pub fn title(&self) -> String {
        self.settings.title(self.model.source())
    }

    /// Applies edited settings (name, width, alerts).
    pub fn set_settings(&mut self, settings: ColumnSettings, cx: &mut Context<Self>) {
        if settings.width != self.settings.width {
            // Row heights depend on the width (wrapping, media size).
            self.list.remeasure();
        }
        self.settings = settings;
        cx.notify();
    }

    pub fn header_hovered(&self) -> bool {
        self.header_hovered
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
        let change = self.model.finish(kind, result);
        self.announce(kind, &change, cx);
        self.mark_arrivals(&change, cx);
        match change {
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

    /// Plays the sound and shows a system notification for new rows, as the
    /// column's alert settings ask.
    fn announce(&self, kind: LoadKind, change: &ListChange, cx: &mut Context<Self>) {
        let alerts = self.settings.alerts;
        if !alerts.sound && !alerts.desktop {
            return;
        }
        let Some(news) = alerts::announce(kind, change, self.model.items()) else {
            return;
        };
        tracing::debug!(column = self.id.0, count = news.count, "announcing new rows");
        if alerts.sound {
            sound::play(alerts.volume, cx);
        }
        if alerts.desktop {
            // One tag per column: a newer batch replaces the column's last toast.
            cx.show_system_notification(SystemNotification {
                tag: format!("column-{}", self.id.0).into(),
                title: self.title().into(),
                body: news.body.into(),
                actions: Vec::new(),
            });
        }
    }

    /// Remembers the rows `change` brings in, so they animate as they appear.
    fn mark_arrivals(&mut self, change: &ListChange, cx: &Context<Self>) {
        if matches!(change, ListChange::Reset { .. }) {
            self.arrivals.clear();
        }
        if cx.reduce_motion() {
            return;
        }
        let now = Instant::now();
        for (ix, entrance) in motion::entrances(change) {
            if let Some(item) = self.model.items().get(ix) {
                self.arrivals.mark(item.key(), entrance, now, entrance.total());
            }
        }
    }

    /// Rows still animating in.
    pub fn arriving(&self) -> usize {
        self.arrivals.len()
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

    /// Switches the column to another variant of its source (a header tab).
    pub fn switch_source(&mut self, source: FeedSource, cx: &mut Context<Self>) {
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
        let rcx = RowCtx { ix, column_width: self.settings.width.px(), hub: self.hub.clone() };
        let row = match self.model.items().get(ix) {
            Some(item) if self.active && self.model.selected() == Some(ix) => div()
                .bg(cx.theme().list_active)
                .child(render_item(item, &rcx, window, cx))
                .into_any_element(),
            Some(item) => render_item(item, &rcx, window, cx),
            None => return self.render_footer(cx),
        };
        let Some(key) = self.model.items().get(ix).map(|item| item.key()) else {
            return row;
        };
        match self.arrivals.get(&key, Instant::now()) {
            Some(&entrance) => arriving(&key, entrance, row, cx.theme().primary.opacity(0.12)),
            None => row,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Marks the column as the target of keyboard shortcuts.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            cx.notify();
        }
    }

    /// Moves the keyboard selection and scrolls it into view.
    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(ix) = self.model.select(delta) {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
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

    fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let source = self.model.source();
        let icon = source_icon(source);
        let tabs = sources::tabs(source);
        let id = self.id;
        let header_button = |name: &'static str, icon: IconName, tip: &'static str| {
            Button::new(name).ghost().small().icon(Icon::new(icon)).tooltip(tip)
        };
        // The buttons fade in while the header is hovered. Hidden ones keep
        // their place, so the title doesn't reflow; they can only be clicked
        // while hovered, and then they show.
        let actions_opacity = base::transition(
            (ElementId::from(("column-actions", id.0 as usize)), "opacity"),
            if self.header_hovered { 1.0_f32 } else { 0.0 },
            base::Transition::new(theme.motion.duration_fast),
            window,
            cx,
        );
        v_flex()
            .id(("column-header", id.0 as usize))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.header_hovered != *hovered {
                    this.header_hovered = *hovered;
                    cx.notify();
                }
            }))
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .when(self.active, |d| d.bg(theme.list_hover))
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
                            .child(self.title()),
                    )
                    .child(
                        h_flex()
                            .debug_selector(|| "column-actions".into())
                            .flex_none()
                            .opacity(actions_opacity)
                            .child(
                                header_button("refresh", IconName::RefreshCw, "Refresh").on_click(
                                    cx.listener(|this, _, _, cx| this.load(LoadKind::Refresh, cx)),
                                ),
                            )
                            .child(
                                header_button("left", IconName::ChevronLeft, "Move left").on_click(
                                    cx.listener(move |_, _, _, cx| {
                                        cx.emit(ColumnEvent::Move(id, -1))
                                    }),
                                ),
                            )
                            .child(
                                header_button("right", IconName::ChevronRight, "Move right")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ColumnEvent::Move(id, 1))
                                    })),
                            )
                            .child(
                                header_button(
                                    "settings",
                                    IconName::SlidersHorizontal,
                                    "Column settings",
                                )
                                .on_click(cx.listener(
                                    move |_, _, _, cx| cx.emit(ColumnEvent::OpenSettings(id)),
                                )),
                            )
                            .child(header_button("remove", IconName::X, "Remove column").on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(ColumnEvent::Remove(id))),
                            )),
                    ),
            )
            .when(!tabs.is_empty(), |d| d.child(self.render_tabs(tabs, cx)))
    }

    fn render_tabs(&self, tabs: Vec<Tab>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.model.source().clone();
        let tab = |Tab { label, source }: Tab| {
            let active = source == current;
            v_flex()
                .id(label)
                .flex_1()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.switch_source(source.clone(), cx)))
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
        h_flex().children(tabs.into_iter().map(tab))
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
                let pill = Button::new("new-posts")
                    .primary()
                    .small()
                    .icon(Icon::new(IconName::ArrowUp))
                    .label(SharedString::from(format!(
                        "{unseen} new post{}",
                        if unseen == 1 { "" } else { "s" }
                    )))
                    .on_click(cx.listener(|this, _, _, cx| this.scroll_to_top(cx)));
                let tokens = theme.motion.clone();
                // Drops in from the column's top edge when it first appears.
                d.child(
                    h_flex().absolute().w_full().justify_center().child(pill).with_animation(
                        "new-posts-in",
                        Animation::new(tokens.duration_slow)
                            .with_easing(move |t| tokens.easing_enter.sample(t)),
                        |d, t| {
                            let t = t.clamp(0., 1.);
                            d.top(px(PILL_TOP - motion::TRAVEL * (1. - t))).opacity(t)
                        },
                    ),
                )
            })
            .into_any_element()
    }
}

/// `row` fading (and sliding) in; newer posts also carry a fading `tint`.
/// Only paint moves: the row's measured height never changes.
fn arriving(key: &str, entrance: Entrance, row: AnyElement, tint: gpui_kit::Hsla) -> AnyElement {
    div()
        .relative()
        .child(row)
        .with_animation(
            SharedString::from(format!("arrive-{key}")),
            Animation::new(entrance.total()),
            move |d, t| {
                let frame = entrance.frame(t);
                d.opacity(frame.opacity)
                    .top(px(frame.offset))
                    .when(frame.tint > 0., |d| d.bg(tint.opacity(frame.tint)))
            },
        )
        .into_any_element()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .flex_none()
            .w(px(self.settings.width.px()))
            .h_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_header(window, cx))
            .child(self.render_body(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, TestAppContext, point};
    use twitty_model::{HomeMode, mock::MockBackend};

    fn setup(cx: &mut TestAppContext) -> (MockBackend, Entity<ColumnView>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend = MockBackend::new();
        let shared: Arc<dyn Backend> = Arc::new(backend.clone());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let column = cx.new(|cx| {
            let config =
                ColumnConfig::new(ColumnId(1), FeedSource::Home { mode: HomeMode::Following });
            ColumnView::new(&config, shared, hub, false, cx)
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
    fn arriving_rows_animate_in(cx: &mut TestAppContext) {
        let (backend, column) = setup(cx);
        cx.run_until_parked();
        let first_page = column.read_with(cx, |c, _| c.arriving());
        assert!(first_page > 0, "the first page cascades in");
        assert!(first_page <= motion::STAGGER_ROWS * 2, "only its first screenful");

        backend.push_new(&FeedSource::Home { mode: HomeMode::Following }, 2);
        column.update(cx, |c, cx| c.load(LoadKind::Newer, cx));
        cx.run_until_parked();
        column.read_with(cx, |c, _| {
            let key = c.model().items()[0].key();
            assert_eq!(c.arrivals.get(&key, Instant::now()), Some(&Entrance::Newer));
        });

        cx.update(|cx| cx.set_reduce_motion(true));
        column.update(cx, |c, cx| c.load(LoadKind::Refresh, cx));
        cx.run_until_parked();
        assert_eq!(
            column.read_with(cx, |c, _| c.arriving()),
            0,
            "no animation with reduced motion"
        );
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
                &ColumnConfig::new(ColumnId(1), FeedSource::Bookmarks),
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
    fn switching_tabs_reloads_and_emits(cx: &mut TestAppContext) {
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
        column.update(cx, |c, cx| c.switch_source(FeedSource::Home { mode: HomeMode::ForYou }, cx));
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

    fn set_alerts(
        column: &Entity<ColumnView>,
        alerts: crate::state::deck::ColumnAlerts,
        cx: &mut TestAppContext,
    ) {
        column.update(cx, |c, cx| {
            let settings = ColumnSettings { alerts, ..c.settings().clone() };
            c.set_settings(settings, cx)
        });
    }

    fn poll_new(backend: &MockBackend, column: &Entity<ColumnView>, cx: &mut TestAppContext) {
        backend.push_new(&FeedSource::Home { mode: HomeMode::Following }, 2);
        column.update(cx, |c, cx| c.load(LoadKind::Newer, cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn new_posts_play_the_sound_and_notify_as_configured(cx: &mut TestAppContext) {
        use crate::{
            sound::RecordingSound,
            state::deck::{ColumnAlerts, Volume},
        };
        let (backend, column) = setup(cx);
        let sound = cx.update(RecordingSound::install);
        // Like Windows, GPUI only posts notifications for an identified app.
        cx.update(|cx| cx.set_app_identity("Twitty.Test", "Twitty"));
        cx.run_until_parked();
        assert!(sound.played().is_empty(), "the first page is not news");

        // Off by default.
        poll_new(&backend, &column, cx);
        assert!(sound.played().is_empty());
        assert!(cx.shown_system_notifications().is_empty());

        let volume = Volume::from(30);
        set_alerts(&column, ColumnAlerts { sound: true, volume, desktop: true }, cx);
        column.update(cx, |c, cx| {
            let mut settings = c.settings().clone();
            settings.set_name("Friends");
            c.set_settings(settings, cx);
        });
        poll_new(&backend, &column, cx);
        assert_eq!(sound.played(), [volume]);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title.as_ref(), "Friends", "titled with the column's name");
        assert!(shown[0].body.starts_with("2 new posts\n"), "{}", shown[0].body);

        // A refresh is not news.
        column.update(cx, |c, cx| c.load(LoadKind::Refresh, cx));
        cx.run_until_parked();
        assert_eq!(sound.played().len(), 1);

        // Sound only.
        set_alerts(&column, ColumnAlerts { sound: true, volume, desktop: false }, cx);
        poll_new(&backend, &column, cx);
        assert_eq!(sound.played().len(), 2);
        assert_eq!(cx.shown_system_notifications().len(), 1);
    }

    #[gpui_kit::test]
    fn header_buttons_show_while_the_header_is_hovered(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let shared: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&shared)));
        let (column, cx) = cx.add_window_view(|_, cx| {
            let config = ColumnConfig::new(ColumnId(1), FeedSource::Bookmarks);
            ColumnView::new(&config, shared, hub, false, cx)
        });
        cx.run_until_parked();
        let over_posts = gpui_kit::point(px(150.), px(400.));
        cx.simulate_mouse_move(over_posts, None, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        assert!(!column.read_with(cx, |c, _| c.header_hovered()));

        let actions = cx.debug_bounds("column-actions").expect("header buttons are laid out");
        cx.simulate_mouse_move(actions.center(), None, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        assert!(column.read_with(cx, |c, _| c.header_hovered()));

        cx.simulate_mouse_move(over_posts, None, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        assert!(!column.read_with(cx, |c, _| c.header_hovered()), "hidden again over the posts");
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
            ColumnView::new(&ColumnConfig::new(ColumnId(1), source), shared, hub, false, cx)
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
