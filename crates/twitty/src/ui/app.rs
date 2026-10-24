//! The window's root view: navigation rail + horizontally scrolling deck.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement, Render, ScrollHandle,
    Styled, Subscription, Task, Window, assets::IconName, component::button::*,
    component::scroll::ScrollableElement, component::*, div, prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedSource, Profile, mock::MockBackend};

use crate::{
    account::{AccountServices, SessionSource},
    runtime::Runtime,
    state::{
        deck::{ColumnConfig, ColumnId, DeckFile},
        settings::Settings,
    },
    ui::{
        add_column::{AddColumnEvent, AddColumnView},
        column::{ColumnEvent, ColumnView},
        hub::{ActionFailed, InteractionsHub},
        login::{LoginEvent, LoginView},
        theme,
    },
};

/// Height of the strip under the deck reserved for its horizontal scrollbar.
const DECK_SCROLLBAR_GUTTER: f32 = 12.0;

/// Shown when the session check at startup gets `Unauthorized`.
pub const EXPIRED_NOTICE: &str = "Your X session expired or was revoked. Sign in again.";

/// Options decided at startup.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Where to persist decks; `None` keeps them in memory only.
    pub deck_path: Option<PathBuf>,
    /// Where to persist settings; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// Whether columns poll for new posts.
    pub poll: bool,
    /// Where the backend's session came from; `None` means demo data.
    pub session: Option<SessionSource>,
    /// Sign-in support; `None` hides the account controls.
    pub account: Option<AccountServices>,
}

/// The root view.
pub struct TwittyApp {
    deck: DeckFile,
    settings: Settings,
    options: AppOptions,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    columns: Vec<(ColumnId, Entity<ColumnView>)>,
    theme_name: &'static str,
    deck_scroll: ScrollHandle,
    /// The signed-in account, once the session check answered.
    viewer: Option<Profile>,
    login: Option<Entity<LoginView>>,
    add_column: Option<Entity<AddColumnView>>,
    account_panel: bool,
    session_check: Option<Task<()>>,
    _login_sub: Option<Subscription>,
    _add_sub: Option<Subscription>,
    _hub_sub: Option<Subscription>,
    /// Column event subscriptions; replaced with the columns.
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for TwittyApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwittyApp").field("deck", &self.deck).finish_non_exhaustive()
    }
}

impl TwittyApp {
    /// The app on `backend`. With a session (`options.session`), its account
    /// is checked right away; an expired session opens the sign-in dialog.
    pub fn new(
        deck: DeckFile,
        settings: Settings,
        backend: Arc<dyn Backend>,
        options: AppOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme_name = settings.theme_or_default(&theme::NAMES).unwrap_or(theme::NAMES[0]);
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&backend)));
        let mut this = Self {
            deck,
            settings,
            options,
            backend,
            hub,
            columns: Vec::new(),
            theme_name,
            deck_scroll: ScrollHandle::new(),
            viewer: None,
            login: None,
            add_column: None,
            account_panel: false,
            session_check: None,
            _login_sub: None,
            _add_sub: None,
            _hub_sub: None,
            _subs: Vec::new(),
        };
        this.build_hub(cx);
        this.build_columns(cx);
        if this.options.session.is_some() {
            this.check_session(window, cx);
        }
        this
    }

    pub fn deck(&self) -> &DeckFile {
        &self.deck
    }

    /// Where the current session came from; `None` in demo mode.
    pub fn session(&self) -> Option<&SessionSource> {
        self.options.session.as_ref()
    }

    pub fn viewer(&self) -> Option<&Profile> {
        self.viewer.as_ref()
    }

    pub fn login(&self) -> Option<&Entity<LoginView>> {
        self.login.as_ref()
    }

    /// (Re)creates the hub on the current backend, dropping the old account's
    /// like/repost/bookmark overrides.
    fn build_hub(&mut self, cx: &mut Context<Self>) {
        self.hub = cx.new(|_| InteractionsHub::new(Arc::clone(&self.backend)));
        self._hub_sub = Some(cx.subscribe(&self.hub, |_, _, e: &ActionFailed, _| {
            tracing::warn!("action failed: {}", e.0);
        }));
    }

    /// (Re)creates every column of the active deck. Old views are dropped,
    /// which cancels their loads and polling.
    fn build_columns(&mut self, cx: &mut Context<Self>) {
        self._subs.clear();
        self.columns.clear();
        let configs = self.deck.active().columns.clone();
        for config in configs {
            let view = self.make_column(&config, cx);
            self.columns.push((config.id, view));
        }
        cx.notify();
    }

    fn check_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let result = Runtime::global(cx).spawn(async move { backend.viewer().await });
        self.session_check = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update_in(cx, |this, window, cx| this.session_checked(result, window, cx)).ok();
        }));
    }

    fn session_checked(
        &mut self,
        result: Result<Profile, BackendError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(viewer) => {
                tracing::info!("signed in as @{}", viewer.handle);
                self.viewer = Some(viewer);
            }
            Err(BackendError::Unauthorized) => {
                tracing::warn!("the stored X session was rejected");
                self.open_login(Some(EXPIRED_NOTICE.to_owned()), window, cx);
            }
            // Columns show their own errors; the session may still be fine.
            Err(e) => tracing::warn!("could not check the X session: {e}"),
        }
        cx.notify();
    }

    /// Shows the sign-in dialog (no-op without account services).
    pub fn open_login(
        &mut self,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(services) = self.options.account.clone() else {
            return;
        };
        let view = cx.new(|cx| LoginView::new(services, notice, window, cx));
        self._login_sub =
            Some(cx.subscribe_in(&view, window, |this, _, e: &LoginEvent, _, cx| match e {
                LoginEvent::SignedIn { backend, viewer } => {
                    this.signed_in(Arc::clone(backend), Profile::clone(viewer), cx);
                }
                LoginEvent::Dismissed => this.close_overlays(cx),
            }));
        self.login = Some(view);
        self.account_panel = false;
        cx.notify();
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        self._login_sub = None;
        self.add_column = None;
        self._add_sub = None;
        self.account_panel = false;
        cx.notify();
    }

    pub fn add_column_dialog(&self) -> Option<&Entity<AddColumnView>> {
        self.add_column.as_ref()
    }

    /// Shows the "Add column" dialog.
    pub fn open_add_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let view = cx.new(|cx| AddColumnView::new(backend, window, cx));
        self._add_sub = Some(cx.subscribe(&view, |this, _, e: &AddColumnEvent, cx| {
            if let AddColumnEvent::Add(source) = e {
                this.add_column(source.clone(), cx);
            }
            this.close_overlays(cx);
        }));
        self.add_column = Some(view);
        self.account_panel = false;
        cx.notify();
    }

    /// Switches to deck `index`.
    pub fn switch_deck(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.deck.active_index() || !self.deck.set_active(index) {
            return;
        }
        self.deck_scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        self.build_columns(cx);
        self.persist(cx);
    }

    /// Adds an empty deck and switches to it.
    pub fn add_deck(&mut self, cx: &mut Context<Self>) {
        self.deck.add_deck();
        self.deck_scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        self.build_columns(cx);
        self.persist(cx);
    }

    /// Removes deck `index` (never the last one).
    pub fn remove_deck(&mut self, index: usize, cx: &mut Context<Self>) {
        let active = self.deck.active_index();
        if self.deck.remove_deck(index) {
            if index == active || self.deck.active_index() != active {
                self.build_columns(cx);
            }
            self.persist(cx);
            cx.notify();
        }
    }

    /// Switches to a session that signed in through the dialog.
    pub fn signed_in(
        &mut self,
        backend: Arc<dyn Backend>,
        viewer: Profile,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("signed in as @{}", viewer.handle);
        self.options.session = Some(SessionSource::Keyring);
        self.viewer = Some(viewer);
        self.session_check = None;
        self.backend = backend;
        self.close_overlays(cx);
        self.build_hub(cx);
        self.build_columns(cx);
    }

    /// Forgets the stored session and goes back to demo data. Only sessions
    /// the app stored itself can be removed.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if self.options.session != Some(SessionSource::Keyring) {
            return;
        }
        if let Some(account) = &self.options.account
            && let Err(e) = account.store.delete()
        {
            tracing::error!("could not remove the stored session: {e}");
        }
        self.options.session = None;
        self.viewer = None;
        self.session_check = None;
        self.backend = Arc::new(MockBackend::new());
        self.close_overlays(cx);
        self.build_hub(cx);
        self.build_columns(cx);
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
        self.settings.theme = Some(self.theme_name.to_owned());
        if let Some(path) = self.options.settings_path.clone() {
            let settings = self.settings.clone();
            cx.background_spawn(async move {
                if let Err(e) = settings.save(&path) {
                    tracing::error!("saving settings to {} failed: {e}", path.display());
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn render_deck_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.deck.active_index();
        let removable = self.deck.decks.len() > 1;
        let tabs = self.deck.decks.iter().enumerate().map(|(ix, deck)| {
            let is_active = ix == active;
            h_flex()
                .id(("deck-tab", ix))
                .h_full()
                .px_3()
                .gap_1()
                .items_center()
                .cursor_pointer()
                .border_b_2()
                .border_color(if is_active { theme.primary } else { theme.transparent })
                .text_color(if is_active { theme.foreground } else { theme.muted_foreground })
                .font_weight(if is_active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.switch_deck(ix, cx)))
                .child(deck.name.clone())
                .when(is_active && removable, |d| {
                    d.child(
                        Button::new(("deck-remove", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Remove deck")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_deck(ix, cx);
                            })),
                    )
                })
        });
        h_flex()
            .flex_none()
            .h(px(36.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .children(tabs)
            .child(
                Button::new("deck-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("New deck")
                    .on_click(cx.listener(|this, _, _, cx| this.add_deck(cx))),
            )
    }

    fn render_account_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.options.account.as_ref()?;
        let button = Button::new("account").ghost().large().w(px(52.)).h(px(52.));
        Some(if self.options.session.is_some() {
            let tip = match &self.viewer {
                Some(v) => format!("Signed in as @{}", v.handle),
                None => "Account".to_owned(),
            };
            button
                .icon(Icon::new(IconName::CircleUser).size(px(26.)))
                .tooltip(tip)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.account_panel = !this.account_panel;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            button
                .icon(Icon::new(IconName::LogIn).size(px(26.)))
                .tooltip("Sign in")
                .on_click(cx.listener(|this, _, window, cx| this.open_login(None, window, cx)))
                .into_any_element()
        })
    }

    fn render_account_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let who = match &self.viewer {
            Some(v) => format!("Signed in as @{}", v.handle),
            None => "Signed in".to_owned(),
        };
        let (detail, can_sign_out) = match &self.options.session {
            Some(SessionSource::Keyring) => {
                ("Stored in Windows Credential Manager.".to_owned(), true)
            }
            Some(SessionSource::External(from)) => {
                (format!("Session from {from}. Remove it there to sign out."), false)
            }
            None => (String::new(), false),
        };
        v_flex()
            .w(px(360.))
            .gap_3()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .child(div().text_lg().font_weight(FontWeight::BOLD).child(who))
            .child(div().text_sm().text_color(theme.muted_foreground).child(detail))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("account-close")
                            .ghost()
                            .label("Close")
                            .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                    )
                    .when(can_sign_out, |d| {
                        d.child(
                            Button::new("sign-out")
                                .danger()
                                .icon(IconName::LogOut)
                                .label("Sign out")
                                .on_click(cx.listener(|this, _, _, cx| this.sign_out(cx))),
                        )
                    }),
            )
            .into_any_element()
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
            .child(
                rail_button("add-column", IconName::Plus, "Add column")
                    .on_click(cx.listener(|this, _, window, cx| this.open_add_column(window, cx))),
            )
            .child(div().flex_1())
            .children(self.render_account_button(cx))
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
            // The scrollbar is an overlay on this wrapper (not on the scrolling
            // element, whose children move with the scroll). The bottom padding
            // gives it its own strip so it never covers the columns.
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .pb(px(DECK_SCROLLBAR_GUTTER))
                .child(
                    h_flex()
                        .id("deck")
                        .size_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.deck_scroll)
                        // Without this GPUI turns a vertical wheel over a column into a
                        // horizontal deck scroll as well. Shift+wheel / touchpad still
                        // scroll sideways.
                        .restrict_scroll_to_axis()
                        .children(self.columns.iter().map(|(_, view)| view.clone())),
                )
                .horizontal_scrollbar(&self.deck_scroll)
                .into_any_element()
        };
        let overlay = if let Some(login) = &self.login {
            Some(login.clone().into_any_element())
        } else if let Some(add) = &self.add_column {
            Some(add.clone().into_any_element())
        } else if self.account_panel {
            Some(self.render_account_panel(cx))
        } else {
            None
        };
        let demo = self.options.session.is_none();
        let can_sign_in = self.options.account.is_some();
        v_flex()
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .when(demo, |d| {
                d.child(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .items_center()
                        .gap_3()
                        .py_1()
                        .text_xs()
                        .bg(theme.primary.opacity(0.15))
                        .text_color(theme.primary)
                        .child("Demo mode: showing generated posts")
                        .when(can_sign_in, |d| {
                            d.child(
                                Button::new("banner-sign-in").xsmall().label("Sign in").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.open_login(None, window, cx)
                                    }),
                                ),
                            )
                        }),
                )
            })
            .child(
                h_flex().flex_1().min_h_0().child(self.render_rail(cx)).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.render_deck_bar(cx))
                        .child(div().flex_1().min_h_0().flex().child(body)),
                ),
            )
            .when_some(overlay, |d, overlay| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .bg(theme.overlay)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(overlay),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{ConnectError, CredentialStore, Credentials, MemoryStore};
    use gpui_kit::{
        Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point,
    };

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
        let options = AppOptions::default();
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, options, window, cx)
        });
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

    fn services(store: &MemoryStore, signed_in: &MockBackend) -> AccountServices {
        let backend = signed_in.clone();
        AccountServices {
            connect: Arc::new(move |_: &Credentials| -> Result<Arc<dyn Backend>, ConnectError> {
                Ok(Arc::new(backend.clone()))
            }),
            store: Arc::new(store.clone()),
        }
    }

    fn start<'a>(
        cx: &'a mut TestAppContext,
        backend: MockBackend,
        session: Option<SessionSource>,
        store: &MemoryStore,
    ) -> (Entity<TwittyApp>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let options = AppOptions {
            session,
            account: Some(services(store, &MockBackend::new())),
            ..AppOptions::default()
        };
        let shared: Arc<dyn Backend> = Arc::new(backend);
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), Settings::default(), shared, options, window, cx)
        });
        cx.run_until_parked();
        (app, cx)
    }

    fn column_entities(
        app: &Entity<TwittyApp>,
        cx: &mut VisualTestContext,
    ) -> Vec<gpui_kit::EntityId> {
        app.read_with(cx, |app, _| app.columns().map(|c| c.entity_id()).collect())
    }

    #[gpui_kit::test]
    fn sign_in_from_demo_stores_session_and_rebuilds_columns(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let before = column_entities(&app, cx);

        app.update_in(cx, |app, window, cx| app.open_login(None, window, cx));
        let login = app.read_with(cx, |app, _| app.login().cloned()).unwrap();
        login.update_in(cx, |l, window, cx| l.set_values("tok", "csrf", window, cx));
        login.update(cx, |l, cx| l.submit(cx));
        cx.run_until_parked();

        app.read_with(cx, |app, _| {
            assert_eq!(app.session(), Some(&SessionSource::Keyring));
            assert!(app.login().is_none(), "dialog closed");
            assert!(app.viewer().is_some());
        });
        assert!(store.load().unwrap().is_some(), "stored");
        let after = column_entities(&app, cx);
        assert_eq!(after.len(), before.len());
        assert!(after.iter().all(|id| !before.contains(id)), "columns rebuilt on the new backend");
    }

    #[gpui_kit::test]
    fn sign_out_clears_store_and_returns_to_demo(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: "a".into(), ct0: "c".into() }).unwrap();
        let (app, cx) = start(cx, MockBackend::new(), Some(SessionSource::Keyring), &store);
        app.update(cx, |app, cx| app.sign_out(cx));
        assert_eq!(store.load().unwrap(), None);
        app.read_with(cx, |app, _| {
            assert_eq!(app.session(), None);
            assert!(app.viewer().is_none());
        });
    }

    #[gpui_kit::test]
    fn external_sessions_are_not_signed_out(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: "a".into(), ct0: "c".into() }).unwrap();
        let source = SessionSource::External("session.json".into());
        let (app, cx) = start(cx, MockBackend::new(), Some(source.clone()), &store);
        app.update(cx, |app, cx| app.sign_out(cx));
        assert!(store.load().unwrap().is_some());
        assert_eq!(app.read_with(cx, |app, _| app.session().cloned()), Some(source));
    }

    #[gpui_kit::test]
    fn expired_session_at_startup_opens_the_dialog(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        // An expired session fails every call: the column loads and the check.
        for _ in 0..=DeckFile::default().active().columns.len() {
            backend.fail_next(BackendError::Unauthorized);
        }
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: "a".into(), ct0: "c".into() }).unwrap();
        let (app, cx) = start(cx, backend, Some(SessionSource::Keyring), &store);
        let login = app.read_with(cx, |app, _| app.login().cloned()).expect("dialog open");
        let notice = login.read_with(cx, |l, _| l.form().notice().map(str::to_owned));
        assert_eq!(notice.as_deref(), Some(EXPIRED_NOTICE));
        assert!(store.load().unwrap().is_some(), "kept until a new sign-in replaces it");
    }

    #[gpui_kit::test]
    fn working_session_at_startup_knows_the_viewer(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), Some(SessionSource::Keyring), &store);
        assert!(app.read_with(cx, |app, _| app.viewer().is_some()));
        assert!(app.read_with(cx, |app, _| app.login().is_none()));
    }

    #[gpui_kit::test]
    fn add_column_dialog_appends_to_the_active_deck(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let before = app.read_with(cx, |app, _| app.deck().active().columns.len());
        app.update_in(cx, |app, window, cx| app.open_add_column(window, cx));
        let dialog = app.read_with(cx, |app, _| app.add_column_dialog().cloned()).unwrap();
        dialog.update_in(cx, |d, window, cx| {
            d.choose(crate::state::add_column::ColumnKind::Bookmarks, window, cx)
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.add_column_dialog().is_none(), "dialog closed");
            let columns = &app.deck().active().columns;
            assert_eq!(columns.len(), before + 1);
            assert_eq!(columns.last().map(|c| &c.source), Some(&FeedSource::Bookmarks));
            assert_eq!(app.columns().count(), before + 1);
        });
    }

    #[gpui_kit::test]
    fn decks_switch_their_columns(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let main_columns = column_entities(&app, cx);
        app.update(cx, |app, cx| app.add_deck(cx));
        assert_eq!(app.read_with(cx, |app, _| app.columns().count()), 0, "new deck is empty");
        app.update(cx, |app, cx| app.add_column(FeedSource::Bookmarks, cx));
        app.update(cx, |app, cx| app.switch_deck(0, cx));
        let back = column_entities(&app, cx);
        assert_eq!(back.len(), main_columns.len());
        app.update(cx, |app, cx| app.remove_deck(1, cx));
        app.read_with(cx, |app, _| {
            assert_eq!(app.deck().decks.len(), 1);
            assert_eq!(app.columns().count(), main_columns.len());
        });
    }
}
