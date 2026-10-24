//! The window's root view: navigation rail + horizontally scrolling deck.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::{
    Context, Entity, FontWeight, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    Styled, Subscription, Window, assets::IconName, component::button::*, component::*, div,
    prelude::*, px,
};
use twitty_model::{Backend, FeedSource, HomeMode, NotificationFilter, SearchSort};

use crate::{
    state::deck::{ColumnConfig, ColumnId, DeckFile},
    ui::{
        column::{ColumnEvent, ColumnView},
        hub::{ActionFailed, InteractionsHub},
        theme,
    },
};

/// Options decided at startup.
#[derive(Debug, Clone)]
pub struct AppOptions {
    /// Where to persist decks; `None` keeps them in memory only.
    pub deck_path: Option<PathBuf>,
    /// Whether columns poll for new posts.
    pub poll: bool,
    /// Shown in a banner, e.g. "Demo data".
    pub banner: Option<SharedString>,
}

/// The root view.
pub struct TwittyApp {
    deck: DeckFile,
    options: AppOptions,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    columns: Vec<(ColumnId, Entity<ColumnView>)>,
    theme_name: &'static str,
    deck_scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for TwittyApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwittyApp").field("deck", &self.deck).finish_non_exhaustive()
    }
}

impl TwittyApp {
    pub fn new(
        deck: DeckFile,
        backend: Arc<dyn Backend>,
        options: AppOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&backend)));
        let hub_sub = cx.subscribe(&hub, |_, _, e: &ActionFailed, _| {
            tracing::warn!("action failed: {}", e.0);
        });
        let mut this = Self {
            deck,
            options,
            backend,
            hub,
            columns: Vec::new(),
            theme_name: theme::NAMES[0],
            deck_scroll: ScrollHandle::new(),
            _subs: vec![hub_sub],
        };
        let configs = this.deck.active().columns.clone();
        for config in configs {
            let view = this.make_column(&config, cx);
            this.columns.push((config.id, view));
        }
        this
    }

    pub fn deck(&self) -> &DeckFile {
        &self.deck
    }

    pub fn columns(&self) -> impl Iterator<Item = &Entity<ColumnView>> {
        self.columns.iter().map(|(_, view)| view)
    }

    fn make_column(&mut self, config: &ColumnConfig, cx: &mut Context<Self>) -> Entity<ColumnView> {
        let backend = Arc::clone(&self.backend);
        let hub = self.hub.clone();
        let poll = self.options.poll;
        let (id, source, width) = (config.id, config.source.clone(), config.width);
        let view = cx.new(|cx| ColumnView::new(id, source, width, backend, hub, poll, cx));
        self._subs.push(cx.subscribe(&view, Self::on_column_event));
        view
    }

    /// Adds a column for `source` at the end of the deck.
    pub fn add_column(&mut self, source: FeedSource, cx: &mut Context<Self>) {
        let id = self.deck.add_column(source);
        if let Some(config) = self.deck.active().columns.iter().find(|c| c.id == id).cloned() {
            let view = self.make_column(&config, cx);
            self.columns.push((id, view));
        }
        self.persist(cx);
        cx.notify();
    }

    fn on_column_event(
        &mut self,
        column: Entity<ColumnView>,
        event: &ColumnEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ColumnEvent::Remove(id) => {
                self.deck.remove_column(*id);
                self.columns.retain(|(cid, _)| cid != id);
            }
            ColumnEvent::Move(id, delta) => {
                self.deck.move_column(*id, *delta);
                self.sync_order();
            }
            ColumnEvent::CycleWidth(id) => {
                if let Some(config) = self.deck.column_mut(*id) {
                    config.width = config.width.cycle();
                    let width = config.width;
                    column.update(cx, |c, cx| c.set_width(width, cx));
                }
            }
            ColumnEvent::SourceChanged(id, source) => {
                if let Some(config) = self.deck.column_mut(*id) {
                    config.source = source.clone();
                }
            }
        }
        self.persist(cx);
        cx.notify();
    }

    /// Reorders the column views to match the deck file.
    fn sync_order(&mut self) {
        let order: Vec<_> = self.deck.active().columns.iter().map(|c| c.id).collect();
        self.columns
            .sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    }

    fn persist(&self, cx: &mut Context<Self>) {
        let Some(path) = self.options.deck_path.clone() else {
            return;
        };
        let deck = self.deck.clone();
        cx.background_spawn(async move {
            if let Err(e) = deck.save(&path) {
                tracing::error!("saving decks to {} failed: {e}", path.display());
            }
        })
        .detach();
    }

    fn cycle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_name = theme::next(self.theme_name);
        theme::apply(self.theme_name, Some(window), cx);
        cx.notify();
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rail_button = |name: &'static str, icon: IconName, tip: &'static str| {
            Button::new(name)
                .ghost()
                .large()
                .icon(Icon::new(icon).size(px(26.)))
                .tooltip(tip)
                .w(px(52.))
                .h(px(52.))
        };
        v_flex()
            .flex_none()
            .w(px(68.))
            .h_full()
            .items_center()
            .py_2()
            .gap_1()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .size(px(52.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(26.))
                    .font_weight(FontWeight::BLACK)
                    .child("𝕏"),
            )
            .child(rail_button("add-home", IconName::House, "Add Home column").on_click(
                cx.listener(|this, _, _, cx| {
                    this.add_column(FeedSource::Home { mode: HomeMode::Following }, cx)
                }),
            ))
            .child(rail_button("add-search", IconName::Search, "Add Search column").on_click(
                cx.listener(|this, _, _, cx| {
                    this.add_column(
                        FeedSource::Search { query: "rust".into(), sort: SearchSort::Latest },
                        cx,
                    )
                }),
            ))
            .child(rail_button("add-notifs", IconName::Bell, "Add Notifications column").on_click(
                cx.listener(|this, _, _, cx| {
                    this.add_column(
                        FeedSource::Notifications { filter: NotificationFilter::All },
                        cx,
                    )
                }),
            ))
            .child(
                rail_button("add-bookmarks", IconName::Bookmark, "Add Bookmarks column").on_click(
                    cx.listener(|this, _, _, cx| this.add_column(FeedSource::Bookmarks, cx)),
                ),
            )
            .child(div().flex_1())
            .child(
                rail_button("theme", IconName::SunMoon, "Switch theme")
                    .on_click(cx.listener(|this, _, window, cx| this.cycle_theme(window, cx))),
            )
            .child(
                Button::new("compose")
                    .primary()
                    .icon(Icon::new(IconName::PenLine).size(px(22.)))
                    .tooltip("Post (coming soon)")
                    .rounded_full()
                    .w(px(52.))
                    .h(px(52.)),
            )
    }
}

impl Render for TwittyApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = if self.columns.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(div().text_xl().font_weight(FontWeight::BOLD).child("No columns"))
                .child("Use the icons on the left to add one.")
                .into_any_element()
        } else {
            h_flex()
                .id("deck")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_x_scroll()
                .track_scroll(&self.deck_scroll)
                // Without this GPUI turns a vertical wheel over a column into a
                // horizontal deck scroll as well. Shift+wheel / touchpad still scroll sideways.
                .restrict_scroll_to_axis()
                .children(self.columns.iter().map(|(_, view)| view.clone()))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .when_some(self.options.banner.clone(), |d, banner| {
                d.child(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .py_1()
                        .text_xs()
                        .bg(theme.primary.opacity(0.15))
                        .text_color(theme.primary)
                        .child(banner),
                )
            })
            .child(h_flex().flex_1().min_h_0().child(self.render_rail(cx)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point};
    use twitty_model::mock::MockBackend;

    #[gpui_kit::test]
    fn vertical_wheel_over_a_column_does_not_scroll_the_deck(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::runtime::Runtime::inline());
        });
        // Enough columns to overflow any test window horizontally.
        let mut deck = DeckFile::default();
        for _ in 0..8 {
            deck.add_column(FeedSource::Bookmarks);
        }
        let options = AppOptions { deck_path: None, poll: false, banner: None };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|_, cx| TwittyApp::new(deck, backend, options, cx));
        cx.run_until_parked();
        let deck_x = |cx: &mut gpui_kit::VisualTestContext| {
            app.read_with(cx, |app, _| app.deck_scroll.offset().x)
        };
        let over_column = point(px(250.), px(500.));

        for _ in 0..5 {
            cx.simulate_event(ScrollWheelEvent {
                position: over_column,
                delta: ScrollDelta::Lines(point(0.0, -3.0)),
                ..Default::default()
            });
            cx.run_until_parked();
        }
        assert_eq!(deck_x(cx), px(0.), "vertical wheel must not move the deck sideways");

        // Shift+wheel arrives from the platform as a horizontal delta.
        cx.simulate_event(ScrollWheelEvent {
            position: over_column,
            delta: ScrollDelta::Lines(point(-3.0, 0.0)),
            modifiers: Modifiers { shift: true, ..Default::default() },
            ..Default::default()
        });
        cx.run_until_parked();
        assert!(deck_x(cx) < px(0.), "horizontal wheel still scrolls the deck");
    }
}
