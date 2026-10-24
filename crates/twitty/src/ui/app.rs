//! The window's root view: navigation rail + horizontally scrolling deck.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, IntoElement, ObjectFit, ParentElement,
    Render, ScrollHandle, Styled, StyledImage, Subscription, Task, Window, assets::IconName,
    component::button::*, component::checkbox::Checkbox, component::scroll::ScrollableElement,
    component::*, div, img, prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedSource, Profile, mock::MockBackend};

use crate::{
    account::{AccountServices, SessionSource},
    runtime::Runtime,
    state::{
        deck::{ColumnConfig, ColumnId, DeckFile},
        interactions::Toggle,
        media_layout::{self, viewer},
        poll,
        settings::Settings,
    },
    ui::{
        add_column::{AddColumnEvent, AddColumnView},
        column::{ColumnEvent, ColumnView},
        compose::{ComposeEvent, ComposeView},
        hub::{ActionFailed, InteractionsHub, Navigate},
        keys,
        login::{LoginEvent, LoginView},
        theme,
        thread::{CloseThread, ThreadPanel},
    },
};

/// Opacity of the theme's overlay colour behind the image viewer, dialogs and
/// the thread panel.
const BACKDROP_ALPHA: f32 = 0.7;

/// Height of the strip under the deck reserved for its horizontal scrollbar.
const DECK_SCROLLBAR_GUTTER: f32 = 12.0;

/// How often polling intervals are recomputed from the backend's limits.
const BUDGET_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// Shown when the session check at startup gets `Unauthorized`.
pub const EXPIRED_NOTICE: &str = "Your X session expired or was revoked. Sign in again.";

/// Options decided at startup.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Where to persist decks; `None` keeps them in memory only.
    pub deck_path: Option<PathBuf>,
    /// Where to persist settings; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// The log file, shown in settings.
    pub log_path: Option<PathBuf>,
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
    compose: Option<Entity<ComposeView>>,
    thread: Option<Entity<ThreadPanel>>,
    account_panel: bool,
    settings_panel: bool,
    /// The image viewer: a post and the index of the media shown.
    media: Option<(Arc<twitty_model::Post>, usize)>,
    /// Index of the column keyboard shortcuts act on.
    active_column: usize,
    focus: FocusHandle,
    session_check: Option<Task<()>>,
    _budget_task: Option<Task<()>>,
    _login_sub: Option<Subscription>,
    _add_sub: Option<Subscription>,
    _compose_sub: Option<Subscription>,
    _thread_sub: Option<Subscription>,
    _hub_subs: Vec<Subscription>,
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
            compose: None,
            thread: None,
            account_panel: false,
            settings_panel: false,
            media: None,
            active_column: 0,
            focus: cx.focus_handle(),
            session_check: None,
            _budget_task: None,
            _login_sub: None,
            _add_sub: None,
            _compose_sub: None,
            _thread_sub: None,
            _hub_subs: Vec::new(),
            _subs: Vec::new(),
        };
        this.build_hub(window, cx);
        this.build_columns(cx);
        if this.options.session.is_some() {
            this.check_session(window, cx);
        }
        if this.options.poll {
            // Real limits arrive with the first responses; re-spread polling.
            this._budget_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(BUDGET_REFRESH).await;
                    if this.update(cx, |this, cx| this.update_polling(cx)).is_err() {
                        break;
                    }
                }
            }));
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
    fn build_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hub = cx.new(|_| InteractionsHub::new(Arc::clone(&self.backend)));
        self._hub_subs = vec![
            cx.subscribe(&self.hub, |_, _, e: &ActionFailed, _| {
                tracing::warn!("action failed: {}", e.0);
            }),
            cx.subscribe_in(&self.hub, window, |this, _, e: &Navigate, window, cx| {
                this.navigate(e.clone(), window, cx);
            }),
        ];
        // Panels show the old account's data.
        self.thread = None;
        self._thread_sub = None;
    }

    pub fn thread_panel(&self) -> Option<&Entity<ThreadPanel>> {
        self.thread.as_ref()
    }

    pub fn compose(&self) -> Option<&Entity<ComposeView>> {
        self.compose.as_ref()
    }

    /// Handles a request from a post row.
    pub fn navigate(&mut self, to: Navigate, window: &mut Window, cx: &mut Context<Self>) {
        match to {
            Navigate::Thread(id) => self.open_thread(id, cx),
            Navigate::Reply(post) => self.open_compose(Some(post), window, cx),
            Navigate::Profile(p) => {
                let source = crate::state::add_column::user_source(p.id.clone(), &p.handle);
                self.add_column(source, cx);
            }
            Navigate::Media { post, index } => {
                if index < post.media.len() {
                    self.media = Some((post, index));
                    cx.notify();
                }
            }
        }
    }

    /// The post and media index shown in the image viewer.
    pub fn media_viewer(&self) -> Option<(&twitty_model::Post, usize)> {
        self.media.as_ref().map(|(p, ix)| (p.as_ref(), *ix))
    }

    pub fn active_column(&self) -> usize {
        self.active_column
    }

    fn active_view(&self) -> Option<Entity<ColumnView>> {
        self.columns.get(self.active_column).map(|(_, v)| v.clone())
    }

    /// Makes column `index` (clamped) the target of keyboard shortcuts.
    fn set_active_column(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active_column = index.min(self.columns.len().saturating_sub(1));
        for (ix, (_, column)) in self.columns.iter().enumerate() {
            let active = ix == self.active_column;
            column.update(cx, |c, cx| c.set_active(active, cx));
        }
        self.deck_scroll.scroll_to_item(self.active_column);
        cx.notify();
    }

    fn has_dialog(&self) -> bool {
        self.login.is_some()
            || self.add_column.is_some()
            || self.compose.is_some()
            || self.account_panel
            || self.settings_panel
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.media.is_none()
            && let Some(column) = self.active_view()
        {
            column.update(cx, |c, cx| c.move_selection(delta, cx));
        }
    }

    fn step_left_right(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some((post, index)) = &mut self.media {
            let len = post.media.len();
            *index = (*index as isize + delta).rem_euclid(len.max(1) as isize) as usize;
            cx.notify();
            return;
        }
        let target = self.active_column.saturating_add_signed(delta);
        self.set_active_column(target, cx);
    }

    fn selected_post(&self, cx: &mut Context<Self>) -> Option<Arc<twitty_model::Post>> {
        if self.media.is_some() {
            return None;
        }
        let column = self.active_view()?;
        column.read(cx).model().selected_post().cloned().map(Arc::new)
    }

    fn toggle_selected(&mut self, toggle: Toggle, cx: &mut Context<Self>) {
        if let Some(post) = self.selected_post(cx) {
            self.hub.update(cx, |hub, cx| hub.toggle(&post, toggle, cx));
        }
    }

    /// Closes the topmost thing: image viewer, dialog, then thread panel.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.media.take().is_some() {
        } else if self.has_dialog() {
            self.close_overlays(cx);
        } else {
            self.close_thread(cx);
        }
        cx.notify();
    }

    fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.thread = None;
        self._thread_sub = None;
        cx.notify();
    }

    fn render_media_viewer(
        &self,
        post: &twitty_model::Post,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let count = post.media.len();
        let Some(media) = post.media.get(index) else {
            return div().into_any_element();
        };
        let playable = !matches!(media, twitty_model::Media::Image { .. });
        let url = post.url.to_string();
        let nav = |id: &'static str, icon: IconName, delta: isize| {
            Button::new(id)
                .ghost()
                .large()
                .w(px(viewer::NAV))
                .icon(Icon::new(icon).size(px(28.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Not a click on the backdrop.
                    cx.stop_propagation();
                    this.step_left_right(delta, cx)
                }))
        };
        let viewport = window.viewport_size();
        let (box_w, box_h) =
            media_layout::viewer_box(viewport.width.into(), viewport.height.into(), count > 1);
        v_flex()
            .id("media-viewer")
            .size_full()
            .p(px(viewer::PADDING))
            // Buttons and the image stop their clicks; anything else is backdrop.
            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .gap(px(viewer::GAP))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(viewer::HEADER))
                    .justify_between()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} / {count} · @{}", index + 1, post.author.handle))
                    .child(
                        Button::new("media-close")
                            .ghost()
                            .icon(IconName::X)
                            .tooltip("Close (Esc)")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(viewer::GAP))
                    .items_center()
                    .justify_center()
                    .when(count > 1, |d| d.child(nav("media-prev", IconName::ChevronLeft, -1)))
                    .child(
                        // An explicit box from the window size, so the image can't
                        // push past the screen edge. The image itself has no size:
                        // GPUI then uses the bitmap's own size, capped by the box,
                        // so it is shrunk to fit but never blown up (pixelated).
                        h_flex()
                            .flex_none()
                            .w(px(box_w))
                            .h(px(box_h))
                            .items_center()
                            .justify_center()
                            .child(
                                img(media.preview().url.as_str().to_owned())
                                    .id("media-image")
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .max_w(px(box_w))
                                    .max_h(px(box_h))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .when(count > 1, |d| d.child(nav("media-next", IconName::ChevronRight, 1))),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h(px(viewer::FOOTER))
                    .overflow_hidden()
                    .justify_center()
                    .gap_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .children(
                        media
                            .alt()
                            .map(|alt| div().max_w(px(640.)).line_clamp(2).child(alt.to_owned())),
                    )
                    .when(playable, |d| {
                        d.child(
                            Button::new("media-play")
                                .primary()
                                .icon(IconName::Play)
                                .label("Play on x.com")
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    cx.open_url(&url);
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Shows the conversation around `id` in the side panel.
    pub fn open_thread(&mut self, id: twitty_model::PostId, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let hub = self.hub.clone();
        let panel = cx.new(|cx| ThreadPanel::new(id, backend, hub, cx));
        self._thread_sub =
            Some(cx.subscribe(&panel, |this, _, _: &CloseThread, cx| this.close_thread(cx)));
        self.thread = Some(panel);
        cx.notify();
    }

    /// Shows the compose dialog, for a reply when `reply_to` is set.
    pub fn open_compose(
        &mut self,
        reply_to: Option<Arc<twitty_model::Post>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = Arc::clone(&self.backend);
        let view = cx.new(|cx| ComposeView::new(backend, reply_to, window, cx));
        self._compose_sub = Some(cx.subscribe(&view, |this, _, e: &ComposeEvent, cx| {
            if let ComposeEvent::Published(post) = e {
                tracing::info!("published post {}", post.id);
                // A reply shows up in the open conversation.
                let parent = post.reply_to.as_ref().and_then(|r| r.post.as_ref());
                if let Some(panel) = &this.thread
                    && parent.is_some_and(|p| p == panel.read(cx).root())
                {
                    panel.update(cx, |panel, cx| panel.reload(cx));
                }
            }
            this.close_overlays(cx);
        }));
        self.compose = Some(view);
        self.account_panel = false;
        cx.notify();
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
        self.update_polling(cx);
        let active = self.active_column;
        self.set_active_column(active, cx);
        cx.notify();
    }

    /// Spreads polling over the columns within the backend's rate limits.
    /// Called whenever the deck changes and periodically, as the backend
    /// learns its real limits.
    pub fn update_polling(&self, cx: &mut Context<Self>) {
        let sources: Vec<FeedSource> =
            self.columns.iter().map(|(_, c)| c.read(cx).model().source().clone()).collect();
        let enabled = self.options.poll && self.settings.polling_enabled();
        let backend = Arc::clone(&self.backend);
        let intervals = poll::intervals(&sources, |s| backend.rate_budget(s));
        for ((_, column), every) in self.columns.iter().zip(intervals) {
            column.update(cx, |c, _| c.set_poll_interval(every.filter(|_| enabled)));
        }
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
            Some(cx.subscribe_in(&view, window, |this, _, e: &LoginEvent, window, cx| match e {
                LoginEvent::SignedIn { backend, viewer } => {
                    this.signed_in(Arc::clone(backend), Profile::clone(viewer), window, cx);
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
        self.compose = None;
        self._compose_sub = None;
        self.account_panel = false;
        self.settings_panel = false;
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("signed in as @{}", viewer.handle);
        self.options.session = Some(SessionSource::Keyring);
        self.viewer = Some(viewer);
        self.session_check = None;
        self.backend = backend;
        self.close_overlays(cx);
        self.build_hub(window, cx);
        self.build_columns(cx);
    }

    /// Forgets the stored session and goes back to demo data. Only sessions
    /// the app stored itself can be removed.
    pub fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.build_hub(window, cx);
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
        self.update_polling(cx);
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
                let active = self.active_column;
                self.set_active_column(active, cx);
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
        if matches!(event, ColumnEvent::Remove(_) | ColumnEvent::SourceChanged(..)) {
            self.update_polling(cx);
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

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn theme_name(&self) -> &'static str {
        self.theme_name
    }

    /// Shows the settings page.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.close_overlays(cx);
        self.settings_panel = true;
        cx.notify();
    }

    /// Switches to the theme called `name` (one of `theme::NAMES`) and saves it.
    pub fn set_theme(&mut self, name: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_name = name;
        theme::apply(name, Some(window), cx);
        self.settings.theme = Some(name.to_owned());
        self.save_settings(cx);
    }

    /// Turns automatic checks for new posts on or off, and saves the choice.
    pub fn set_polling(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.polling = Some(enabled);
        self.update_polling(cx);
        self.save_settings(cx);
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
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

    fn render_settings_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let section =
            |title: &'static str| div().pt_2().text_sm().font_weight(FontWeight::BOLD).child(title);
        let current = self.theme_name;
        let themes = theme::NAMES.into_iter().map(|name| {
            let button = Button::new(name)
                .small()
                .label(name)
                .on_click(cx.listener(move |this, _, window, cx| this.set_theme(name, window, cx)));
            if name == current { button.primary() } else { button.ghost() }
        });
        let weak = cx.entity().downgrade();
        let shortcuts = keys::HELP.into_iter().map(|(keys, what)| {
            h_flex()
                .gap_3()
                .text_sm()
                .child(div().w(px(150.)).text_color(theme.muted_foreground).child(keys))
                .child(what)
        });
        let folder = self
            .options
            .deck_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(|p| p.display().to_string());
        v_flex()
            .id("settings-page")
            .w(px(520.))
            .max_h(px(840.))
            .overflow_y_scroll()
            .gap_2()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Settings"))
            .child(section("Theme"))
            .child(h_flex().gap_2().children(themes))
            .child(section("Timelines"))
            .child(
                Checkbox::new("polling")
                    .checked(self.settings.polling_enabled())
                    .label("Check for new posts automatically (within X's rate limits)")
                    .on_click(move |checked, _, cx| {
                        weak.update(cx, |this, cx| this.set_polling(*checked, cx)).ok();
                    }),
            )
            .child(section("Keyboard shortcuts"))
            .child(v_flex().gap_1().children(shortcuts))
            .when_some(folder, |d, folder| {
                d.child(section("Data")).child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("Decks and settings are saved in {folder}")),
                )
            })
            .child(section("Diagnostics"))
            .child(self.render_health(cx))
            .when_some(self.options.log_path.clone(), |d, log| {
                d.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_1().truncate().child(format!("Log: {}", log.display())))
                        .child(
                            Button::new("open-log")
                                .xsmall()
                                .label("Show")
                                .on_click(move |_, _, cx| cx.reveal_path(&log)),
                        ),
                )
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("settings-close")
                        .label("Done")
                        .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                ),
            )
            .into_any_element()
    }

    /// One line per request type: responses, rows, rows Twitty couldn't read,
    /// errors, and the last problem. Unreadable rows mean X changed its data.
    fn render_health(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::state::diagnostics::{self, Verdict};
        let theme = cx.theme().clone();
        let health = self.backend.health();
        let verdict = diagnostics::verdict(&health);
        let color = match verdict {
            Verdict::Drift { .. } => theme.warning,
            Verdict::Nothing | Verdict::Healthy => theme.muted_foreground,
        };
        v_flex()
            .gap_1()
            .text_xs()
            .child(div().text_sm().text_color(color).child(verdict.text()))
            .children(health.into_iter().map(|h| {
                let problem = h.last_error.clone().or_else(|| h.last_warning.clone());
                let color = if diagnostics::flagged(&h) { theme.warning } else { theme.foreground };
                v_flex()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_color(color)
                            .child(div().w(px(170.)).truncate().child(h.name.clone()))
                            .child(diagnostics::line(&h)),
                    )
                    .children(problem.map(|p| {
                        div().pl_4().truncate().text_color(theme.muted_foreground).child(p)
                    }))
            }))
            .into_any_element()
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
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.sign_out(window, cx)),
                                ),
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
                rail_button("settings", IconName::Settings, "Settings")
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            )
            .child(
                Button::new("compose")
                    .primary()
                    .icon(Icon::new(IconName::PenLine).size(px(22.)))
                    .tooltip("Post")
                    .rounded_full()
                    .w(px(52.))
                    .h(px(52.))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_compose(None, window, cx)),
                    ),
            )
    }
}

impl Render for TwittyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Give the keyboard back to the deck once no dialog holds it.
        if !self.has_dialog() && !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus, cx);
        }
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
        let backdrop = theme.overlay.alpha(BACKDROP_ALPHA);
        let overlay = if let Some((post, index)) = &self.media {
            Some(self.render_media_viewer(post, *index, window, cx))
        } else if let Some(login) = &self.login {
            Some(login.clone().into_any_element())
        } else if let Some(add) = &self.add_column {
            Some(add.clone().into_any_element())
        } else if let Some(compose) = &self.compose {
            Some(compose.clone().into_any_element())
        } else if self.account_panel {
            Some(self.render_account_panel(cx))
        } else if self.settings_panel {
            Some(self.render_settings_panel(cx))
        } else {
            None
        };
        let has_overlay = overlay.is_some();
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
                h_flex()
                    .id("deck-root")
                    .relative()
                    .track_focus(&self.focus)
                    .key_context(keys::DECK)
                    .on_action(cx.listener(|this, _: &keys::SelectNext, _, cx| this.select(1, cx)))
                    .on_action(
                        cx.listener(|this, _: &keys::SelectPrevious, _, cx| this.select(-1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &keys::ColumnLeft, _, cx| {
                            this.step_left_right(-1, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &keys::ColumnRight, _, cx| {
                            this.step_left_right(1, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &keys::LikeSelected, _, cx| {
                        this.toggle_selected(Toggle::Like, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::RepostSelected, _, cx| {
                        this.toggle_selected(Toggle::Repost, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::BookmarkSelected, _, cx| {
                        this.toggle_selected(Toggle::Bookmark, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::ReplySelected, window, cx| {
                        if let Some(post) = this.selected_post(cx) {
                            this.open_compose(Some(post), window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::OpenSelected, _, cx| {
                        if let Some(post) = this.selected_post(cx) {
                            this.open_thread(post.id.clone(), cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::NewPost, window, cx| {
                        if this.media.is_none() {
                            this.open_compose(None, window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::Dismiss, _, cx| this.dismiss(cx)))
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_deck_bar(cx))
                            .child(div().flex_1().min_h_0().flex().child(body)),
                    )
                    .when_some(self.thread.clone(), |d, panel| {
                        // A sheet over the deck's right side. Its backdrop takes
                        // the whole click that closes it, so nothing under it
                        // (a post, an image, a link) reacts. With an overlay
                        // open, that overlay's backdrop darkens everything.
                        d.when(!has_overlay, |d| {
                            d.child(
                                div()
                                    .id("thread-backdrop")
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .occlude()
                                    .bg(backdrop)
                                    .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_thread(cx);
                                    })),
                            )
                        })
                        .child(div().absolute().top_0().right_0().h_full().occlude().child(panel))
                    }),
            )
            .when_some(overlay, |d, overlay| {
                d.child(
                    div()
                        .key_context(keys::OVERLAY)
                        .on_action(cx.listener(|this, _: &keys::Dismiss, _, cx| this.dismiss(cx)))
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .bg(backdrop)
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
        Modifiers, Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
        point,
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
        app.update_in(cx, |app, window, cx| app.sign_out(window, cx));
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
        app.update_in(cx, |app, window, cx| app.sign_out(window, cx));
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

    fn first_post(app: &Entity<TwittyApp>, cx: &mut VisualTestContext) -> Arc<twitty_model::Post> {
        let column = app.read_with(cx, |app, _| app.columns().next().cloned()).unwrap();
        column.read_with(cx, |c, _| {
            Arc::new(c.model().items().iter().flat_map(|i| i.posts()).next().unwrap().clone())
        })
    }

    #[gpui_kit::test]
    fn clicking_a_post_opens_its_thread(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let post = first_post(&app, cx);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(post.id.clone()), cx));
        cx.run_until_parked();
        let panel = app.read_with(cx, |app, _| app.thread_panel().cloned()).expect("panel open");
        assert_eq!(panel.read_with(cx, |p, _| p.root().clone()), post.id);
        assert!(panel.read_with(cx, |p, _| matches!(
            p.state(),
            crate::state::thread::ThreadState::Loaded(_)
        )));
    }

    #[gpui_kit::test]
    fn replying_publishes_and_closes_the_dialog(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let post = first_post(&app, cx);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Reply(Arc::clone(&post)), cx));
        let compose = app.read_with(cx, |app, _| app.compose().cloned()).expect("compose open");
        assert_eq!(
            compose.read_with(cx, |c, _| c.reply_to().map(|p| p.id.clone())),
            Some(post.id.clone())
        );
        compose.update_in(cx, |c, window, cx| c.type_text("nice", window, cx));
        compose.update(cx, |c, cx| c.submit(cx));
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.compose().is_none()), "closed after posting");
    }

    #[gpui_kit::test]
    fn profile_card_adds_a_user_column(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let post = first_post(&app, cx);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Profile(Arc::clone(&post.author)), cx));
        cx.run_until_parked();
        let last = app.read_with(cx, |app, _| app.deck().active().columns.last().cloned()).unwrap();
        assert_eq!(last.source.title(), format!("@{}", post.author.handle));
    }

    /// A mock whose feeds allow 30 requests per 15 minutes.
    struct Limited(MockBackend);

    impl Backend for Limited {
        fn viewer(&self) -> twitty_model::BoxFuture<'_, Result<Profile, BackendError>> {
            self.0.viewer()
        }
        fn feed<'a>(
            &'a self,
            source: &'a FeedSource,
            page: twitty_model::PageRequest,
        ) -> twitty_model::BoxFuture<'a, Result<twitty_model::FeedPage, BackendError>> {
            self.0.feed(source, page)
        }
        fn thread<'a>(
            &'a self,
            id: &'a twitty_model::PostId,
        ) -> twitty_model::BoxFuture<'a, Result<twitty_model::ThreadView, BackendError>> {
            self.0.thread(id)
        }
        fn profile<'a>(
            &'a self,
            handle: &'a str,
        ) -> twitty_model::BoxFuture<'a, Result<Profile, BackendError>> {
            self.0.profile(handle)
        }
        fn act(
            &self,
            action: twitty_model::PostAction,
        ) -> twitty_model::BoxFuture<'_, Result<(), BackendError>> {
            self.0.act(action)
        }
        fn publish(
            &self,
            draft: twitty_model::Draft,
        ) -> twitty_model::BoxFuture<'_, Result<twitty_model::Post, BackendError>> {
            self.0.publish(draft)
        }
        fn rate_budget(&self, _: &FeedSource) -> Option<twitty_model::RateBudget> {
            Some(twitty_model::RateBudget {
                limit: 30,
                window: std::time::Duration::from_secs(900),
            })
        }
    }

    #[gpui_kit::test]
    fn polling_is_spread_within_the_budget(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let mut deck = DeckFile::default();
        deck.add_column(FeedSource::Bookmarks);
        deck.add_column(FeedSource::Bookmarks);
        let options = AppOptions { poll: true, ..AppOptions::default() };
        let backend: Arc<dyn Backend> = Arc::new(Limited(MockBackend::new()));
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, options, window, cx)
        });
        let intervals = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                app.columns().map(|c| c.read(cx).poll_interval()).collect::<Vec<_>>()
            })
        };
        let got = intervals(cx);
        // 30 / 3 = 10 polls per window; the two Bookmarks columns share them.
        assert_eq!(
            got.iter().filter(|i| **i == Some(std::time::Duration::from_secs(180))).count(),
            2
        );
        assert!(got.iter().all(Option::is_some));

        app.update(cx, |app, cx| {
            app.settings.polling = Some(false);
            app.update_polling(cx);
        });
        assert!(intervals(cx).iter().all(Option::is_none), "polling can be turned off");
    }

    fn keyboard_app(cx: &mut TestAppContext) -> (Entity<TwittyApp>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            keys::bind(cx);
            cx.set_global(Runtime::inline());
        });
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(
                DeckFile::default(),
                Settings::default(),
                backend,
                AppOptions::default(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        // A frame gives the deck keyboard focus.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        (app, cx)
    }

    fn selected(app: &Entity<TwittyApp>, cx: &mut VisualTestContext) -> Option<usize> {
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        column.read_with(cx, |c, _| c.model().selected())
    }

    #[gpui_kit::test]
    fn j_k_and_arrows_move_the_selection_and_column(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("j j j k");
        assert_eq!(selected(&app, cx), Some(1));
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 1);
        cx.simulate_keystrokes("j");
        assert_eq!(selected(&app, cx), Some(0));
        cx.simulate_keystrokes("left left left");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0, "clamped");
    }

    #[gpui_kit::test]
    fn l_likes_the_selected_post_and_enter_opens_it(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("j l");
        cx.run_until_parked();
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let post = column.read_with(cx, |c, _| c.model().selected_post().cloned()).unwrap();
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        assert!(hub.read_with(cx, |hub, _| hub.effective(&post).viewer.liked));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let panel = app.read_with(cx, |app, _| app.thread_panel().cloned()).expect("thread open");
        assert_eq!(panel.read_with(cx, |p, _| p.root().clone()), post.id);
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.thread_panel().is_none()), "Esc closes the panel");
    }

    #[gpui_kit::test]
    fn n_opens_compose_and_typing_there_is_not_a_shortcut(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("n");
        let compose = app.read_with(cx, |app, _| app.compose().cloned()).expect("compose open");
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_keystrokes("j l n");
        assert_eq!(selected(&app, cx), None, "keys went to the text field");
        assert!(app.read_with(cx, |app, _| app.compose().is_some()));
        drop(compose);
    }

    #[gpui_kit::test]
    fn media_viewer_steps_and_closes(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let post = {
            let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
            column.read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| p.media.len() >= 2)
                    .cloned()
                    .map(Arc::new)
            })
        }
        .expect("the mock has a multi-image post");
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let count = post.media.len();
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Media { post, index: 0 }, cx));
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.media_viewer().map(|(_, ix)| ix)), Some(1));
        cx.simulate_keystrokes("left left");
        assert_eq!(
            app.read_with(cx, |app, _| app.media_viewer().map(|(_, ix)| ix)),
            Some(count - 1),
            "wraps around"
        );
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0, "columns don't move");
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_none()));
    }

    /// Opens the thread panel on `root` and draws a frame so it has a hitbox.
    fn open_panel(app: &Entity<TwittyApp>, root: &str, cx: &mut VisualTestContext) {
        let id = twitty_model::PostId::new(root);
        app.update(cx, |app, cx| app.open_thread(id, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn panel_root(app: &Entity<TwittyApp>, cx: &mut VisualTestContext) -> Option<String> {
        let panel = app.read_with(cx, |app, _| app.thread_panel().cloned())?;
        Some(panel.read_with(cx, |p, _| p.root().to_string()))
    }

    /// Somewhere left of the panel, past the rail.
    const OUTSIDE: Point<Pixels> = point(px(300.), px(300.));

    #[gpui_kit::test]
    fn clicking_outside_the_thread_panel_closes_it(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        // An empty deck, so the click lands on nothing but the background.
        app.update(cx, |app, cx| app.add_deck(cx));
        open_panel(&app, "1", cx);
        let viewport = cx.update(|window, _| window.viewport_size());
        assert!(viewport.width > OUTSIDE.x + px(crate::ui::thread::PANEL_WIDTH));

        let inside = point(viewport.width - px(100.), px(300.));
        cx.simulate_click(inside, Modifiers::default());
        cx.run_until_parked();
        assert!(panel_root(&app, cx).is_some(), "a click inside keeps it open");

        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx), None);
    }

    #[gpui_kit::test]
    fn clicks_in_an_overlay_do_not_close_the_thread_panel(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let post = column.read_with(cx, |c, _| {
            c.model().items().iter().flat_map(|i| i.posts()).find(|p| !p.media.is_empty()).cloned()
        });
        let post = Arc::new(post.expect("the mock has a post with media"));
        app.update(cx, |app, cx| app.add_deck(cx));
        open_panel(&app, "1", cx);
        // e.g. an image opened from a post in the panel.
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Media { post, index: 0 }, cx));
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_some()));

        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        assert!(panel_root(&app, cx).is_some());
    }

    #[gpui_kit::test]
    fn the_click_that_closes_the_thread_panel_does_nothing_else(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        // Find a media tile in the first column: a click there opens the viewer.
        let tile = (100..1000)
            .step_by(10)
            .map(|y| point(px(250.), px(y as f32)))
            .find(|&at| {
                cx.simulate_click(at, Modifiers::default());
                cx.run_until_parked();
                let hit = app.read_with(cx, |app, _| app.media_viewer().is_some());
                app.update(cx, |app, cx| {
                    app.dismiss(cx);
                    app.dismiss(cx);
                });
                cx.run_until_parked();
                hit
            })
            .expect("the first column shows an image");

        open_panel(&app, "elsewhere", cx);
        cx.simulate_click(tile, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx), None, "the panel closed");
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_none()), "no image opened");

        // Over the first post's text: it doesn't open its thread either.
        open_panel(&app, "elsewhere", cx);
        cx.simulate_click(point(px(250.), px(200.)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx), None);
    }

    #[gpui_kit::test]
    fn clicking_the_media_backdrop_closes_the_viewer_but_buttons_do_not(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let post = column.read_with(cx, |c, _| {
            c.model().items().iter().flat_map(|i| i.posts()).find(|p| p.media.len() >= 2).cloned()
        });
        let post = Arc::new(post.expect("the mock has a multi-image post"));
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Media { post, index: 0 }, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let viewport = cx.update(|window, _| window.viewport_size());
        let index = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| app.media_viewer().map(|(_, ix)| ix))
        };

        // The › button, centred vertically at the right edge.
        let next =
            point(viewport.width - px(viewer::PADDING + viewer::NAV / 2.), viewport.height / 2.);
        cx.simulate_click(next, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(index(cx), Some(1), "the button steps and keeps the viewer open");

        // The padding around everything is backdrop.
        cx.simulate_click(
            point(px(viewer::PADDING / 2.), px(viewer::PADDING / 2.)),
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(index(cx), None);
    }

    #[gpui_kit::test]
    fn settings_change_theme_and_polling_and_are_saved(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(theme::NAMES[0], cx);
            cx.set_global(Runtime::inline());
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let options =
            AppOptions { poll: true, settings_path: Some(path.clone()), ..AppOptions::default() };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), Settings::default(), backend, options, window, cx)
        });
        app.update(cx, |app, cx| app.open_settings(cx));
        app.update_in(cx, |app, window, cx| app.set_theme("Light", window, cx));
        app.update(cx, |app, cx| app.set_polling(false, cx));
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.theme_name()), "Light");
        let polling = app.read_with(cx, |app, cx| {
            app.columns().map(|c| c.read(cx).poll_interval()).collect::<Vec<_>>()
        });
        assert!(polling.iter().all(Option::is_none));
        let saved = Settings::load(&path).unwrap();
        assert_eq!((saved.theme.as_deref(), saved.polling), (Some("Light"), Some(false)));
        cx.simulate_keystrokes("escape");
    }
}
