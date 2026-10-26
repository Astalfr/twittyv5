//! The window's root view: navigation rail + horizontally scrolling deck.

use std::{path::PathBuf, sync::Arc, time::Instant};

use gpui_kit::{
    Animation, AnimationExt, AnyElement, AnyView, AnyWindowHandle, Context, ElementId, Entity,
    FocusHandle, FontWeight, ImgResourceLoader, IntoElement, ObjectFit, ParentElement, Render,
    Resource, ScrollHandle, SharedString, SharedUri, Styled, StyledImage, Subscription, Task,
    Window,
    assets::IconName,
    canvas,
    component::button::*,
    component::checkbox::Checkbox,
    component::scroll::ScrollableElement,
    component::slider::{Slider, SliderEvent, SliderState},
    component::tooltip::Tooltip,
    component::*,
    div, img,
    prelude::*,
    px,
};
use twitty_model::{
    Backend, BackendError, FeedSource, Profile, SpanKind, UserRef, mock::MockBackend,
};

use crate::{
    account::{AccountServices, SessionSource},
    runtime::Runtime,
    state::{
        compose::ComposeTarget,
        deck::{ColumnConfig, ColumnId, ColumnSettings, DeckFile, DeckIcon},
        interactions::Toggle,
        media_layout::{self, MaxHeight, viewer},
        motion,
        playback::{self, Autoplay, State},
        poll,
        repost::RepostChoice,
        settings::Settings,
        sheets::{self, SheetStack},
        stats,
        toasts::{Notice, Toasts},
        update::{Release, Status as UpdateStatus, Updates, Version},
    },
    ui::button::TextButton,
    ui::{
        add_column::{AddColumnEvent, AddColumnView},
        assets::{ADD_COLUMN, ADD_DECK},
        autoscroll::AutoscrollLayer,
        column::{ColumnEvent, ColumnView},
        column_settings::{ColumnSettingsEvent, ColumnSettingsView},
        compose::{ComposeEvent, ComposeView},
        deck_settings::{self, DeckSettingsEvent, DeckSettingsView, DeckTarget},
        emoji,
        gif::GifAutoplay,
        hub::{InteractionsHub, Navigate},
        keys,
        login::{LoginEvent, LoginView},
        player::{Options as PlayerOptions, Quality, VideoPlayer, play_pause_sign},
        post::{MediaMaxHeight, span_url},
        profile::{CloseProfile, ProfilePanel},
        repost::{RepostEvent, RepostView},
        theme,
        thread::{CloseThread, ThreadPanel},
        title_bar, toasts as toast_view,
        user_search::{UserSearchEvent, UserSearchView},
    },
    update::{UpdateError, UpdateServices},
};

/// Opacity of the theme's overlay colour behind the image viewer, dialogs and
/// the thread panel.
const BACKDROP_ALPHA: f32 = 0.7;
/// The shade over a sheet under the top one, so the top one stands out.
const LOWER_SHEET_ALPHA: f32 = 0.25;

/// Height of the strip under the deck reserved for its horizontal scrollbar.
const DECK_SCROLLBAR_GUTTER: f32 = 12.0;
/// Radius of the deck area's top-left corner, where the title bar and the
/// rail meet.
const CHROME_RADIUS: f32 = 8.0;

/// Width of the sidebar with icons only, and expanded (with labels).
const RAIL_WIDTH: f32 = 68.0;
const RAIL_EXPANDED_WIDTH: f32 = 220.0;
/// Inset of the expanded sidebar's entries, except the decks, which span it.
const RAIL_PADDING: f32 = 8.0;
/// Width of the bar on the left of the selected deck.
const DECK_MARK_WIDTH: f32 = 3.0;
/// Height of a sidebar entry.
const RAIL_ENTRY_HEIGHT: f32 = 48.0;

/// The signed-in account's avatar in the sidebar, in logical pixels.
const ACCOUNT_AVATAR: f32 = 30.0;

/// How often polling intervals are recomputed from the backend's limits.
const BUDGET_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// When the first update check runs after startup, and how often after that.
const UPDATE_FIRST_CHECK: std::time::Duration = std::time::Duration::from_secs(10);
const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Shown when the session check at startup gets `Unauthorized`.
pub const EXPIRED_NOTICE: &str = "Your Twitter session expired or was revoked. Sign in again.";

/// Something that just closed, still drawn while it animates out. The
/// slot is cleared by `_clear` when the exit animation is over.
struct Leaving<T> {
    what: T,
    _clear: Task<()>,
}

/// An overlay that just closed (see [`Leaving`]).
enum LeavingOverlay {
    Media(Arc<twitty_model::Post>, usize),
    /// A dialog view: sign-in, add column, column or deck settings, compose,
    /// retweet, people search.
    View(AnyView),
    Account,
    Settings,
}

/// What the side sheet shows.
#[derive(Clone)]
enum Sheet {
    Thread(Entity<ThreadPanel>),
    Profile(Entity<ProfilePanel>),
}

impl Sheet {
    fn view(&self) -> AnyView {
        match self {
            Sheet::Thread(panel) => panel.clone().into(),
            Sheet::Profile(panel) => panel.clone().into(),
        }
    }

    fn entity_id(&self) -> gpui_kit::EntityId {
        match self {
            Sheet::Thread(panel) => panel.entity_id(),
            Sheet::Profile(panel) => panel.entity_id(),
        }
    }
}

/// A sheet in the stack, with the subscription to its close event.
struct OpenSheet {
    sheet: Sheet,
    _sub: Subscription,
}

/// Options decided at startup.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Where to persist decks; `None` keeps them in memory only.
    pub deck_path: Option<PathBuf>,
    /// Where to persist settings; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// The log file, shown in settings.
    pub log_path: Option<PathBuf>,
    /// Whether columns poll for new tweets.
    pub poll: bool,
    /// Where the backend's session came from; `None` means demo data.
    pub session: Option<SessionSource>,
    /// Sign-in support; `None` hides the account controls.
    pub account: Option<AccountServices>,
    /// Self-update from GitHub Releases; `None` (tests, `cargo run`) never
    /// checks.
    pub updates: Option<UpdateServices>,
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
    /// Whether the columns were wider than the deck at the last frame, so the
    /// scrollbar shows and gets its strip under them.
    deck_overflows: bool,
    /// Middle-click autoscroll, drawn over everything.
    autoscroll: Entity<AutoscrollLayer>,
    /// The settings page's scroll position, kept in a handle so the copy that
    /// animates out after "Done" doesn't jump back to the top.
    settings_scroll: ScrollHandle,
    /// The settings page's media height slider (its last step is no limit).
    media_height: Entity<SliderState>,
    _media_height_sub: Subscription,
    /// The signed-in account, once the session check answered.
    viewer: Option<Profile>,
    login: Option<Entity<LoginView>>,
    add_column: Option<Entity<AddColumnView>>,
    column_settings: Option<Entity<ColumnSettingsView>>,
    deck_settings: Option<Entity<DeckSettingsView>>,
    compose: Option<Entity<ComposeView>>,
    repost: Option<Entity<RepostView>>,
    user_search: Option<Entity<UserSearchView>>,
    /// The side sheets (conversations, profiles), stacked: the top one is
    /// live, the ones under it show their left edge.
    sheets: SheetStack<OpenSheet>,
    /// The lookup of a mentioned account, before its profile opens.
    mention_lookup: Option<Task<()>>,
    account_panel: bool,
    settings_panel: bool,
    /// The latest settings write. Each save waits for the previous one, so the
    /// file always ends up with the newest settings.
    settings_save: Option<Task<()>>,
    /// The latest deck write, chained like `settings_save`.
    deck_save: Option<Task<()>>,
    /// The window the app is the root of, for dialogs opened from column events.
    window: AnyWindowHandle,
    /// The image viewer: a tweet and the index of the media shown.
    media: Option<(Arc<twitty_model::Post>, usize)>,
    /// Plays the viewer's media when it is a GIF or a video.
    media_player: Option<Entity<VideoPlayer>>,
    /// The overlay animating out after it closed. Inert: it gets no input.
    leaving: Option<Leaving<LeavingOverlay>>,
    /// The sheets animating out after they closed, each with its depth in the
    /// stack when it closed, the deepest first. Inert too.
    leaving_sheet: Option<Leaving<Vec<(Sheet, usize)>>>,
    /// When the first sheet opened (not stacked), so the backdrop fades in once.
    sheet_opened: Option<Instant>,
    /// Index of the column keyboard shortcuts act on.
    active_column: usize,
    focus: FocusHandle,
    session_check: Option<Task<()>>,
    _budget_task: Option<Task<()>>,
    /// Refreshes the counters of the tweets on screen.
    _stats_task: Option<Task<()>>,
    _login_sub: Option<Subscription>,
    _add_sub: Option<Subscription>,
    _column_settings_sub: Option<Subscription>,
    _deck_settings_sub: Option<Subscription>,
    _compose_sub: Option<Subscription>,
    _repost_sub: Option<Subscription>,
    _user_search_sub: Option<Subscription>,
    _hub_subs: Vec<Subscription>,
    /// The update on offer, if any.
    updates: Updates,
    /// The periodic update check.
    _update_checks: Option<Task<()>>,
    /// A manual update check ("Check now").
    update_check: Option<Task<()>>,
    /// The download and launch of the installer. Separate from the check, so
    /// "Check now" during a download doesn't cancel it.
    update_install: Option<Task<()>>,
    /// Messages at the bottom of the window, and the timer that hides them.
    toasts: Toasts,
    _toast_timer: Option<Task<()>>,
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
        let media_height = cx.new(|_| {
            SliderState::new()
                // `min` clamps against the current max (100), so max first.
                .max(MaxHeight::slider_value(None))
                .min(f32::from(MaxHeight::MIN))
                .step(f32::from(MaxHeight::STEP))
                .default_value(MaxHeight::slider_value(settings.media_max_height))
        });
        let media_height_sub =
            cx.subscribe(&media_height, |this, _, e: &SliderEvent, cx| match e {
                SliderEvent::Change(v) => {
                    this.set_media_max_height(MaxHeight::from_slider(v.start()), false, cx);
                }
                SliderEvent::Release(v) => {
                    this.set_media_max_height(MaxHeight::from_slider(v.start()), true, cx);
                }
            });
        let mut this = Self {
            deck,
            settings,
            options,
            backend,
            hub,
            columns: Vec::new(),
            theme_name,
            deck_scroll: ScrollHandle::new(),
            deck_overflows: false,
            autoscroll: cx.new(|_| AutoscrollLayer::new()),
            settings_scroll: ScrollHandle::new(),
            media_height,
            _media_height_sub: media_height_sub,
            viewer: None,
            login: None,
            add_column: None,
            column_settings: None,
            deck_settings: None,
            compose: None,
            repost: None,
            user_search: None,
            sheets: SheetStack::default(),
            mention_lookup: None,
            account_panel: false,
            settings_panel: false,
            settings_save: None,
            deck_save: None,
            window: window.window_handle(),
            media: None,
            media_player: None,
            leaving: None,
            leaving_sheet: None,
            sheet_opened: None,
            active_column: 0,
            focus: cx.focus_handle(),
            session_check: None,
            _budget_task: None,
            _stats_task: None,
            _login_sub: None,
            _add_sub: None,
            _column_settings_sub: None,
            _deck_settings_sub: None,
            _compose_sub: None,
            _repost_sub: None,
            _user_search_sub: None,
            _hub_subs: Vec::new(),
            updates: Updates::new(Version::current()),
            _update_checks: None,
            update_check: None,
            update_install: None,
            toasts: Toasts::default(),
            _toast_timer: None,
            _subs: Vec::new(),
        };
        // GPUI never sets this from the OS; only the setting turns motion off.
        if !this.settings.animations_enabled() {
            cx.set_reduce_motion(true);
        }
        cx.set_global(GifAutoplay(this.settings.gif_autoplay()));
        cx.set_global(MediaMaxHeight(this.settings.media_max_height));
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
            this._stats_task = Some(cx.spawn(async move |this, cx| {
                let mut wait = stats::REFRESH_EVERY;
                loop {
                    cx.background_executor().timer(wait).await;
                    let Ok(refresh) = this.update(cx, |this, cx| this.refresh_stats(cx)) else {
                        break;
                    };
                    let result = match refresh {
                        Some(task) => task.await,
                        None => Ok(()),
                    };
                    wait = stats::next_delay(result.as_ref().map(|_| ()), chrono::Utc::now());
                }
            }));
        }
        this.schedule_update_checks(cx);
        this
    }

    /// The update banner's state.
    pub fn updates(&self) -> &Updates {
        &self.updates
    }

    /// Looks for a new release shortly after startup and then every few
    /// hours, while updates are available and the setting is on.
    fn schedule_update_checks(&mut self, cx: &mut Context<Self>) {
        self._update_checks = None;
        if self.options.updates.is_none() || !self.settings.check_updates() {
            return;
        }
        self._update_checks = Some(cx.spawn(async move |this, cx| {
            let mut wait = UPDATE_FIRST_CHECK;
            loop {
                cx.background_executor().timer(wait).await;
                wait = UPDATE_INTERVAL;
                let Ok(Some(latest)) = this.update(cx, |this, cx| this.fetch_latest(cx)) else {
                    break;
                };
                let found = latest.await;
                if this.update(cx, |this, cx| this.latest_found(found, false, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Asks GitHub for the latest release, on the network runtime.
    fn fetch_latest(
        &self,
        cx: &gpui_kit::App,
    ) -> Option<impl Future<Output = Result<Option<Release>, UpdateError>> + use<>> {
        let source = Arc::clone(&self.options.updates.as_ref()?.source);
        let task = Runtime::global(cx).spawn(async move { source.latest().await });
        Some(async move { task.await.unwrap_or_else(|e| Err(UpdateError::Network(e.to_string()))) })
    }

    /// Settings' "Check now": like the periodic check, but it says when
    /// Twitty is up to date or the check failed, and offers a release the
    /// user said "Later" to.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        let Some(latest) = self.fetch_latest(cx) else {
            return;
        };
        self.update_check = Some(cx.spawn(async move |this, cx| {
            let found = latest.await;
            this.update(cx, |this, cx| this.latest_found(found, true, cx)).ok();
        }));
    }

    fn latest_found(
        &mut self,
        found: Result<Option<Release>, UpdateError>,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        match found {
            Ok(latest) => {
                let newer = self.updates.found(latest, manual);
                if manual && !newer {
                    let current = self.updates.current();
                    self.show_toast(Notice::success(format!("Twitty {current} is up to date")), cx);
                }
            }
            Err(e) => {
                tracing::warn!("checking for updates failed: {e}");
                if manual {
                    self.show_toast(Notice::error(format!("Checking for updates failed: {e}")), cx);
                }
            }
        }
        cx.notify();
    }

    /// The banner's "Install": downloads the installer, starts it and quits
    /// once pending writes are done. The installer starts Twitty again.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(services) = self.options.updates.clone() else {
            return;
        };
        let Some(release) = self.updates.start() else {
            return;
        };
        let UpdateServices { source, launch } = services;
        let download = Runtime::global(cx).spawn(async move { source.download(&release).await });
        self.update_install = Some(cx.spawn(async move |this, cx| {
            let downloaded =
                download.await.unwrap_or_else(|e| Err(UpdateError::Network(e.to_string())));
            match downloaded.and_then(|path| launch(&path)) {
                Ok(()) => {
                    tracing::info!("update installer started, quitting");
                    let pending = this
                        .update(cx, |this, _| (this.settings_save.take(), this.deck_save.take()));
                    if let Ok((settings, deck)) = pending {
                        for save in [settings, deck].into_iter().flatten() {
                            save.await;
                        }
                    }
                    cx.update(|cx| cx.quit());
                }
                Err(e) => {
                    tracing::error!("installing the update failed: {e}");
                    this.update(cx, |this, cx| {
                        this.updates.failed(e.to_string());
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
        cx.notify();
    }

    /// The banner's "Later".
    pub fn dismiss_update(&mut self, cx: &mut Context<Self>) {
        self.updates.dismiss();
        cx.notify();
    }

    /// The update banner, under the demo one.
    fn render_update_banner(&self, rounded: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let (text, failed, page) = match self.updates.status() {
            UpdateStatus::Idle => return None,
            UpdateStatus::Available(r) => {
                (format!("Twitty {} is available", r.version), false, Some(r.page.clone()))
            }
            UpdateStatus::Downloading(r) => {
                (format!("Downloading Twitty {}…", r.version), false, None)
            }
            UpdateStatus::Failed(r, why) => {
                (format!("Updating to {} failed: {why}", r.version), true, None)
            }
        };
        let offering = !matches!(self.updates.status(), UpdateStatus::Downloading(_));
        let color = if failed { theme.danger } else { theme.primary };
        Some(
            h_flex()
                .debug_selector(|| "update-banner".into())
                .w_full()
                .justify_center()
                .items_center()
                .gap_3()
                .py_1()
                .text_xs()
                .when(rounded, |d| d.rounded_tl(px(CHROME_RADIUS - 1.)))
                .bg(color.opacity(0.15))
                .text_color(color)
                .child(div().min_w_0().truncate().child(text))
                .when(offering, |d| {
                    d.child(
                        Button::new("update-install")
                            .text_button(Size::XSmall)
                            .label(if failed { "Retry" } else { "Install and restart" })
                            .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                    )
                })
                .when_some(page, |d, page| {
                    d.child(
                        Button::new("update-notes")
                            .text_button(Size::XSmall)
                            .label("What's new")
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                })
                .when(offering, |d| {
                    d.child(
                        Button::new("update-later")
                            .text_button(Size::XSmall)
                            .label("Later")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_update(cx))),
                    )
                })
                .into_any_element(),
        )
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
            cx.subscribe(&self.hub, |this, _, notice: &Notice, cx| {
                this.show_toast(notice.clone(), cx);
            }),
            cx.subscribe_in(&self.hub, window, |this, _, e: &Navigate, window, cx| {
                this.navigate(e.clone(), window, cx);
            }),
        ];
        // Panels show the old account's data.
        self.sheets.clear();
        self.leaving_sheet = None;
        self.mention_lookup = None;
    }

    /// The toasts on screen.
    pub fn toasts(&self) -> &Toasts {
        &self.toasts
    }

    /// Shows `notice` at the bottom of the window for a few seconds.
    pub fn show_toast(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.toasts.push(notice, cx.background_executor().now());
        self.schedule_toasts(cx);
        cx.notify();
    }

    /// Starts fading out toast `id` (it was clicked).
    fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.dismiss(id, cx.background_executor().now());
        self.schedule_toasts(cx);
        cx.notify();
    }

    /// Wakes up when the next toast hides or finishes fading out. Time comes
    /// from the executor, so tests can move it.
    fn schedule_toasts(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let Some(next) = self.toasts.next_change(now) else {
            self._toast_timer = None;
            return;
        };
        let wait = next.saturating_duration_since(now);
        self._toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.toasts.expire(cx.background_executor().now());
                this.schedule_toasts(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The thread panel, when it is the top sheet.
    pub fn thread_panel(&self) -> Option<&Entity<ThreadPanel>> {
        match self.sheets.top().map(|open| &open.sheet) {
            Some(Sheet::Thread(panel)) => Some(panel),
            _ => None,
        }
    }

    /// The profile sheet, when it is the top sheet.
    pub fn profile_panel(&self) -> Option<&Entity<ProfilePanel>> {
        match self.sheets.top().map(|open| &open.sheet) {
            Some(Sheet::Profile(panel)) => Some(panel),
            _ => None,
        }
    }

    /// How many sheets are stacked.
    pub fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    pub fn compose(&self) -> Option<&Entity<ComposeView>> {
        self.compose.as_ref()
    }

    pub fn repost_dialog(&self) -> Option<&Entity<RepostView>> {
        self.repost.as_ref()
    }

    /// Handles a request from a tweet row.
    pub fn navigate(&mut self, to: Navigate, window: &mut Window, cx: &mut Context<Self>) {
        match to {
            Navigate::Thread(id) => self.open_thread(id, cx),
            Navigate::Reply(post) => self.open_compose(ComposeTarget::Reply(post), window, cx),
            Navigate::Repost(post) => self.open_repost(post, window, cx),
            Navigate::Profile(p) => self.open_profile(p, cx),
            Navigate::Mention(user) => self.open_mention(user, cx),
            Navigate::UserColumn(p) => {
                let source = crate::state::add_column::user_source(p.id.clone(), &p.handle);
                self.add_column(source, cx);
            }
            Navigate::Media { post, index } => {
                if index < post.media.len() {
                    self.media = Some((post, index));
                    self.sync_media_player(window, cx);
                    cx.notify();
                }
            }
        }
    }

    /// The tweet and media index shown in the image viewer.
    pub fn media_viewer(&self) -> Option<(&twitty_model::Post, usize)> {
        self.media.as_ref().map(|(p, ix)| (p.as_ref(), *ix))
    }

    /// The player of the viewer's GIF or video.
    pub fn media_player(&self) -> Option<&Entity<VideoPlayer>> {
        self.media_player.as_ref()
    }

    /// Starts playing the viewer's media if it is a GIF (looping, silent) or
    /// a video (once, with sound and controls), and stops whatever played
    /// before.
    fn sync_media_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stream =
            self.media.as_ref().and_then(|(post, index)| match post.media.get(*index)? {
                twitty_model::Media::Gif { stream: Some(url), .. } => {
                    Some((url.to_string(), PlayerOptions::GIF))
                }
                twitty_model::Media::Video { variants, duration, .. } => {
                    let best = variants.first()?.url.to_string();
                    Some((best, PlayerOptions::video(*duration)))
                }
                _ => None,
            });
        let qualities = self.media_qualities();
        self.media_player = stream.map(|(url, options)| {
            cx.new(|cx| VideoPlayer::new(&url, options, window, cx).with_qualities(qualities))
        });
    }

    /// The viewer's video's files for the quality menu, best first.
    fn media_qualities(&self) -> Vec<Quality> {
        let Some((post, index)) = &self.media else { return Vec::new() };
        let Some(twitty_model::Media::Video { variants, .. }) = post.media.get(*index) else {
            return Vec::new();
        };
        variants
            .iter()
            .map(|v| Quality {
                label: playback::quality_label(v.size, v.bitrate).into(),
                url: v.url.to_string(),
            })
            .collect()
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
            || self.column_settings.is_some()
            || self.deck_settings.is_some()
            || self.compose.is_some()
            || self.repost.is_some()
            || self.user_search.is_some()
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

    fn step_left_right(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((post, index)) = &mut self.media {
            let len = post.media.len();
            *index = (*index as isize + delta).rem_euclid(len.max(1) as isize) as usize;
            self.sync_media_player(window, cx);
            cx.notify();
            return;
        }
        let Some(column) = self.active_view() else {
            return;
        };
        // The first arrow press only selects the first tweet.
        if column.read(cx).model().selected().is_none() {
            column.update(cx, |c, cx| c.move_selection(0, cx));
            return;
        }
        let target = self.active_column.saturating_add_signed(delta);
        self.set_active_column(target, cx);
        if let Some(column) = self.active_view()
            && column.read(cx).model().selected().is_none()
        {
            column.update(cx, |c, cx| c.move_selection(0, cx));
        }
    }

    fn selected_post(&self, cx: &mut Context<Self>) -> Option<Arc<twitty_model::Post>> {
        if self.media.is_some() {
            return None;
        }
        let column = self.active_view()?;
        column.read(cx).model().selected_post().cloned().map(Arc::new)
    }

    fn repost_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(post) = self.selected_post(cx) {
            self.open_repost(post, window, cx);
        }
    }

    fn toggle_selected(&mut self, toggle: Toggle, cx: &mut Context<Self>) {
        if let Some(post) = self.selected_post(cx) {
            self.hub.update(cx, |hub, cx| hub.toggle(&post, toggle, cx));
        }
    }

    /// Closes the topmost thing: image viewer, dialog, then the sheet.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        // Esc first ends a middle-click autoscroll.
        if self.autoscroll.update(cx, |layer, cx| layer.stop(cx)) {
            return;
        }
        if let Some((post, index)) = self.media.take() {
            self.media_player = None;
            self.leave(LeavingOverlay::Media(post, index), cx);
        } else if self.has_dialog() {
            self.close_overlays(cx);
        } else if !self.sheets.is_empty() {
            self.close_top_sheet(cx);
        } else {
            self.reset_keyboard(cx);
        }
        cx.notify();
    }

    /// Back to the launch state: no tweet selected, the first column active.
    fn reset_keyboard(&mut self, cx: &mut Context<Self>) {
        for (_, column) in &self.columns {
            column.update(cx, |c, cx| c.clear_selection(cx));
        }
        self.set_active_column(0, cx);
    }

    /// Closes the sheet showing `entity` (its × was clicked).
    fn close_sheet(&mut self, entity: gpui_kit::EntityId, cx: &mut Context<Self>) {
        let was_top = self.sheets.top().is_some_and(|open| open.sheet.entity_id() == entity);
        if let Some(open) = self.sheets.remove(|open| open.sheet.entity_id() == entity) {
            // Only the top sheet is on screen whole; one under it just goes.
            if was_top {
                self.leave_sheets(vec![open], cx);
            }
        }
        cx.notify();
    }

    /// Closes the top sheet (Esc, or a click beside it: on a lower sheet's
    /// edge or the backdrop), showing the one under it.
    fn close_top_sheet(&mut self, cx: &mut Context<Self>) {
        if let Some(top) = self.sheets.pop() {
            self.leave_sheets(vec![top], cx);
        }
    }

    /// Keeps `closed` (sheets that just came off the top of the stack, the
    /// deepest first) drawn while they animate out.
    /// Sheets still sliding out from an earlier close stay, over these: they
    /// were above them.
    fn leave_sheets(&mut self, closed: Vec<OpenSheet>, cx: &mut Context<Self>) {
        let count = closed.len();
        let mut leaving: Vec<_> = closed
            .into_iter()
            .enumerate()
            .map(|(ix, open)| (open.sheet, count - 1 - ix))
            .filter(|(_, depth)| *depth <= sheets::MAX_PEEKS)
            .collect();
        if let Some(earlier) = self.leaving_sheet.take() {
            leaving.extend(earlier.what);
        }
        self.leaving_sheet = (!leaving.is_empty() && !cx.reduce_motion()).then(|| Leaving {
            what: leaving,
            _clear: Self::after_exit(cx, cx.theme().motion.duration_slow, |this| {
                this.leaving_sheet = None
            }),
        });
        cx.notify();
    }

    /// Keeps `what` drawn while it animates out.
    fn leave(&mut self, what: LeavingOverlay, cx: &mut Context<Self>) {
        self.leaving = (!cx.reduce_motion()).then(|| {
            let wait = cx.theme().motion.duration_fast;
            Leaving { what, _clear: Self::after_exit(cx, wait, |this| this.leaving = None) }
        });
    }

    /// Runs `clear` once an exit animation of `wait` is over.
    fn after_exit(
        cx: &mut Context<Self>,
        wait: std::time::Duration,
        clear: fn(&mut Self),
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                clear(this);
                cx.notify();
            })
            .ok();
        })
    }

    /// Whether something is still animating out.
    pub fn is_leaving(&self) -> bool {
        self.leaving.is_some() || self.leaving_sheet.is_some()
    }

    fn render_media_viewer(
        &self,
        post: &twitty_model::Post,
        index: usize,
        interactive: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let count = post.media.len();
        let Some(media) = post.media.get(index) else {
            return div().into_any_element();
        };
        // The GIF playing here, unless it couldn't (the leaving copy shows the
        // poster only).
        let player = self.media_player.as_ref().filter(|p| interactive && !p.read(cx).failed());
        let paused = player.is_some_and(|p| p.read(cx).state() == State::Paused);
        let playable = !matches!(media, twitty_model::Media::Image { .. }) && player.is_none();
        let url = post.url.to_string();
        let viewport = window.viewport_size();
        let (box_w, box_h) =
            media_layout::viewer_box(viewport.width.into(), viewport.height.into(), count > 1);
        let src = media.preview().url.as_str().to_owned();
        // The loaded bitmap's size, from the cache entry the `img` draws.
        let drawn = cx
            .fetch_asset::<ImgResourceLoader>(&Resource::Uri(SharedUri::from(src.clone())))
            .and_then(Result::ok)
            .and_then(|image| {
                let size = image.size(0);
                media_layout::viewer_image_size(
                    (size.width.0 as f32, size.height.0 as f32),
                    (box_w, box_h),
                )
            });
        // Strips along the whole left and right screen edges (over the side
        // padding, header and footer); ghost, so the background only shows on
        // hover. Drawn first, so the header and footer buttons stay on top.
        let nav = |id: &'static str, icon: IconName, delta: isize| {
            Button::new(id)
                .ghost()
                .large()
                .absolute()
                .top_0()
                .map(|b| if delta < 0 { b.left_0() } else { b.right_0() })
                .w(px(viewer::PADDING + viewer::NAV))
                .h_full()
                .rounded_none()
                .icon(Icon::new(icon).size(px(28.)))
                .when(interactive, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        // Not a click on the backdrop.
                        cx.stop_propagation();
                        this.step_left_right(delta, window, cx)
                    }))
                })
        };
        v_flex()
            .id("media-viewer")
            .relative()
            .size_full()
            .py(px(viewer::PADDING))
            // Buttons and the image stop their clicks; anything else is backdrop.
            .when(interactive, |d| d.on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))))
            .gap(px(viewer::GAP))
            .when(count > 1, |d| {
                d.child(nav("media-prev", IconName::ChevronLeft, -1)).child(nav(
                    "media-next",
                    IconName::ChevronRight,
                    1,
                ))
            })
            .child(
                h_flex()
                    .flex_none()
                    .px(px(viewer::PADDING))
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
                            .when(interactive, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(viewer::GAP))
                    .items_center()
                    .justify_center()
                    .px(px(viewer::PADDING))
                    // Room for the ‹ › strips over the sides.
                    .when(count > 1, |d| d.child(div().flex_none().w(px(viewer::NAV))))
                    .child(
                        // An explicit box from the window size, so the image can't
                        // push past the screen edge. The image is the bitmap's
                        // own size shrunk to fit the box, never blown up
                        // (pixelated). It is sized exactly: with only max sizes,
                        // GPUI clamps each side on its own and letterboxes the
                        // picture, and clicks on the bars would not close.
                        h_flex()
                            .flex_none()
                            .relative()
                            .w(px(box_w))
                            .h(px(box_h))
                            .items_center()
                            .justify_center()
                            .child(
                                img(src)
                                    .id("media-image")
                                    .debug_selector(|| "media-image".into())
                                    .when(interactive, |i| {
                                        i.on_click(|_, _, cx| cx.stop_propagation())
                                    })
                                    .map(|i| match drawn {
                                        Some((w, h)) => i.w(px(w)).h(px(h)),
                                        None => i.max_w(px(box_w)).max_h(px(box_h)),
                                    })
                                    .object_fit(ObjectFit::Contain)
                                    .map(|i| {
                                        // Stepping to another image fades it in (not
                                        // the leaving copy: it would restart at 0).
                                        if interactive {
                                            i.with_animation(
                                                ("media-image-in", index),
                                                Animation::new(theme.motion.duration_normal),
                                                |i, t| i.opacity(t),
                                            )
                                            .into_any_element()
                                        } else {
                                            i.into_any_element()
                                        }
                                    }),
                            )
                            .children(player.map(|player| {
                                // Over the poster, at its size (the whole box
                                // until the poster has loaded).
                                let (w, h) = drawn.unwrap_or((box_w, box_h));
                                let toggled = player.clone();
                                div()
                                    .id("media-player")
                                    .debug_selector(|| "media-player".into())
                                    .absolute()
                                    .left(px((box_w - w) / 2.))
                                    .top(px((box_h - h) / 2.))
                                    .w(px(w))
                                    .h(px(h))
                                    .cursor_pointer()
                                    .on_click(move |_, _, cx| {
                                        // A click pauses or resumes; not the backdrop's.
                                        cx.stop_propagation();
                                        toggled.update(cx, |p, cx| p.toggle(cx));
                                    })
                                    .child(player.clone())
                                    .when(paused, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(play_pause_sign(false, 64.)),
                                        )
                                    })
                            })),
                    )
                    .when(count > 1, |d| d.child(div().flex_none().w(px(viewer::NAV)))),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(viewer::PADDING))
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
                                .debug_selector(|| "media-play".into())
                                .primary()
                                .icon(IconName::Play)
                                .text_button(Size::Medium)
                                .label("Play on Twitter")
                                .when(interactive, |b| {
                                    b.on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        cx.open_url(&url);
                                    })
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Shows the conversation around `id` in a sheet over the others, unless
    /// it is already the top one.
    pub fn open_thread(&mut self, id: twitty_model::PostId, cx: &mut Context<Self>) {
        if let Some(panel) = self.thread_panel()
            && *panel.read(cx).root() == id
        {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let hub = self.hub.clone();
        let panel = cx.new(|cx| ThreadPanel::new(id, backend, hub, cx));
        let sub = cx.subscribe(&panel, |this, panel, _: &CloseThread, cx| {
            this.close_sheet(panel.entity_id(), cx)
        });
        self.show_sheet(Sheet::Thread(panel), sub, cx);
    }

    /// Shows `profile` in a sheet over the others, unless it is already the
    /// top one.
    pub fn open_profile(&mut self, profile: Arc<Profile>, cx: &mut Context<Self>) {
        if let Some(panel) = self.profile_panel()
            && panel.read(cx).profile().id == profile.id
        {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let hub = self.hub.clone();
        let viewer = self.viewer.as_ref().map(|v| v.id.clone());
        let panel = cx.new(|cx| ProfilePanel::new(profile, viewer, backend, hub, cx));
        let sub = cx.subscribe(&panel, |this, panel, _: &CloseProfile, cx| {
            this.close_sheet(panel.entity_id(), cx)
        });
        self.show_sheet(Sheet::Profile(panel), sub, cx);
    }

    /// Looks up the account a text mentions, then shows its profile. If it
    /// can't be found (e.g. suspended), x.com shows why.
    pub fn open_mention(&mut self, user: UserRef, cx: &mut Context<Self>) {
        if let Some(panel) = self.profile_panel()
            && panel.read(cx).profile().handle.eq_ignore_ascii_case(&user.handle)
        {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let handle = user.handle.to_string();
        let result = Runtime::global(cx).spawn(async move { backend.profile(&handle).await });
        self.mention_lookup = Some(cx.spawn(async move |this, cx| {
            let error = match result.await {
                Ok(Ok(profile)) => {
                    this.update(cx, |this, cx| this.open_profile(Arc::new(profile), cx)).ok();
                    return;
                }
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            tracing::warn!("profile lookup of a mention failed: {error}");
            cx.update(|cx| cx.open_url(&span_url(&SpanKind::Mention(user))));
        }));
    }

    /// Puts `sheet` on top of the stack.
    fn show_sheet(&mut self, sheet: Sheet, sub: Subscription, cx: &mut Context<Self>) {
        if self.sheets.is_empty() {
            self.sheet_opened = Some(Instant::now());
        }
        self.leaving_sheet = None;
        self.sheets.push(OpenSheet { sheet, _sub: sub });
        cx.notify();
    }

    /// Shows the compose dialog, for a new tweet, a reply or a quote.
    pub fn open_compose(
        &mut self,
        target: ComposeTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = Arc::clone(&self.backend);
        let view = cx.new(|cx| ComposeView::new(backend, target, window, cx));
        self._compose_sub = Some(cx.subscribe(&view, |this, view, e: &ComposeEvent, cx| {
            if let ComposeEvent::Published(post) = e {
                tracing::info!("published tweet {}", post.id);
                // From what was sent: Twitter's answer may leave the quote out.
                let notice = Notice::published(view.read(cx).target());
                this.show_toast(notice, cx);
                // A reply shows up in the open conversation.
                let parent = post.reply_to.as_ref().and_then(|r| r.post.as_ref());
                if let Some(panel) = this.thread_panel()
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

    /// Asks whether to retweet `post` (or undo its retweet), or quote it.
    pub fn open_repost(
        &mut self,
        post: Arc<twitty_model::Post>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hub = self.hub.read(cx);
        let choice = RepostChoice::for_post(&hub.effective(&post));
        let original = hub.showing_original(&post.id);
        let shown = Arc::clone(&post);
        let view = cx.new(|cx| RepostView::new(shown, choice, original, window, cx));
        self._repost_sub =
            Some(cx.subscribe_in(&view, window, move |this, _, e: &RepostEvent, window, cx| {
                this.close_overlays(cx);
                match e {
                    // Only when nothing changed it meanwhile, so the toggle
                    // does what the button said.
                    RepostEvent::Confirm(choice)
                        if RepostChoice::for_post(&this.hub.read(cx).effective(&post))
                            == *choice =>
                    {
                        this.hub.update(cx, |hub, cx| hub.toggle(&post, Toggle::Repost, cx));
                    }
                    RepostEvent::Quote => {
                        this.open_compose(ComposeTarget::Quote(Arc::clone(&post)), window, cx);
                    }
                    RepostEvent::Confirm(_) | RepostEvent::Dismissed => {}
                }
            }));
        self.repost = Some(view);
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

    /// Looks up the tweets on screen in every column again, so their counters
    /// keep moving. `None` when polling is off or nothing is on screen.
    pub fn refresh_stats(&self, cx: &mut Context<Self>) -> Option<Task<Result<(), BackendError>>> {
        if !self.settings.polling_enabled() {
            return None;
        }
        let columns: Vec<_> = self.columns.iter().map(|(_, c)| c.read(cx)).collect();
        let ids = stats::ids(columns.iter().flat_map(|c| c.visible_items()));
        if ids.is_empty() {
            return None;
        }
        Some(self.hub.update(cx, |hub, cx| hub.refresh(ids, cx)))
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
                tracing::warn!("the stored Twitter session was rejected");
                self.open_login(Some(EXPIRED_NOTICE.to_owned()), window, cx);
            }
            // Columns show their own errors; the session may still be fine.
            Err(e) => tracing::warn!("could not check the Twitter session: {e}"),
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
        // The image viewer draws over a dialog, so then nothing visible closes.
        let closing = if self.media.is_some() {
            None
        } else if let Some(view) = &self.login {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.add_column {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.column_settings {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.deck_settings {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.compose {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.repost {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if let Some(view) = &self.user_search {
            Some(LeavingOverlay::View(view.clone().into()))
        } else if self.account_panel {
            Some(LeavingOverlay::Account)
        } else if self.settings_panel {
            Some(LeavingOverlay::Settings)
        } else {
            None
        };
        if let Some(closing) = closing {
            self.leave(closing, cx);
        }
        self.login = None;
        self._login_sub = None;
        self.add_column = None;
        self._add_sub = None;
        // A slider moved by keyboard only previews; closing saves whatever
        // the column was left with.
        if self.column_settings.take().is_some() {
            self.persist(cx);
        }
        self._column_settings_sub = None;
        self.deck_settings = None;
        self._deck_settings_sub = None;
        self.compose = None;
        self._compose_sub = None;
        self.repost = None;
        self._repost_sub = None;
        self.user_search = None;
        self._user_search_sub = None;
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

    pub fn user_search_dialog(&self) -> Option<&Entity<UserSearchView>> {
        self.user_search.as_ref()
    }

    /// Shows the "Search people" dialog; a picked account opens in the sheet.
    pub fn open_user_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let demo = self.options.session.is_none();
        let view = cx.new(|cx| UserSearchView::new(backend, demo, window, cx));
        self._user_search_sub = Some(cx.subscribe(&view, |this, _, e: &UserSearchEvent, cx| {
            this.close_overlays(cx);
            if let UserSearchEvent::Open(profile) = e {
                this.open_profile(Arc::clone(profile), cx);
            }
        }));
        self.user_search = Some(view);
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

    /// Adds an empty deck called `name` (blank: "Deck N") with `icon`, and
    /// switches to it.
    pub fn add_deck(&mut self, name: &str, icon: DeckIcon, cx: &mut Context<Self>) {
        self.deck.add_named_deck(name, icon);
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

    pub fn deck_settings_dialog(&self) -> Option<&Entity<DeckSettingsView>> {
        self.deck_settings.as_ref()
    }

    /// Shows the deck dialog: name and icon of a new deck, or of deck
    /// `index` (which it can also remove).
    pub fn open_deck_settings(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (target, name, icon) = match index {
            None => (DeckTarget::New, self.deck.next_deck_name(), DeckIcon::default()),
            Some(index) => {
                let Some(deck) = self.deck.decks.get(index) else {
                    return;
                };
                let removable = self.deck.decks.len() > 1;
                (DeckTarget::Existing { index, removable }, deck.name.clone(), deck.icon)
            }
        };
        self.close_overlays(cx);
        let view = cx.new(|cx| DeckSettingsView::new(target, &name, icon, window, cx));
        self._deck_settings_sub =
            Some(cx.subscribe(&view, |this, _, e: &DeckSettingsEvent, cx| match e {
                DeckSettingsEvent::Renamed(ix, name) => {
                    if this.deck.rename_deck(*ix, name) {
                        this.persist(cx);
                        cx.notify();
                    }
                }
                DeckSettingsEvent::IconChanged(ix, icon) => {
                    if this.deck.set_deck_icon(*ix, *icon) {
                        this.persist(cx);
                        cx.notify();
                    }
                }
                DeckSettingsEvent::Remove(ix) => {
                    let ix = *ix;
                    this.close_overlays(cx);
                    this.remove_deck(ix, cx);
                }
                DeckSettingsEvent::Create(name, icon) => {
                    let (name, icon) = (name.clone(), *icon);
                    this.close_overlays(cx);
                    this.add_deck(&name, icon, cx);
                }
                DeckSettingsEvent::Dismissed => this.close_overlays(cx),
            }));
        self.deck_settings = Some(view);
        cx.notify();
    }

    /// Whether the sidebar shows labels and the decks' edit buttons.
    pub fn sidebar_expanded(&self) -> bool {
        self.settings.sidebar_expanded()
    }

    /// Expands or collapses the sidebar, and saves the choice.
    pub fn set_sidebar_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.settings.sidebar_expanded = Some(expanded);
        self.save_settings(cx);
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
        let view = cx.new(|cx| ColumnView::new(config, backend, hub, poll, cx));
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
        _: Entity<ColumnView>,
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
            ColumnEvent::OpenSettings(id) => {
                // The dialog's text input needs the window, which column
                // events don't carry: open it once this event is handled.
                let (id, app, window) = (*id, cx.entity().downgrade(), self.window);
                cx.defer(move |cx| {
                    window
                        .update(cx, |_, window, cx| {
                            app.update(cx, |this, cx| this.open_column_settings(id, window, cx))
                        })
                        .ok();
                });
                return;
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

    /// Saves the decks. Each save waits for the previous one, so quick edits
    /// can't interleave their writes and the file ends up with the newest deck.
    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.options.deck_path.clone() else {
            return;
        };
        let deck = self.deck.clone();
        let previous = self.deck_save.take();
        self.deck_save = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(e) = deck.save(&path) {
                tracing::error!("saving decks to {} failed: {e}", path.display());
            }
        }));
    }

    pub fn column_settings_dialog(&self) -> Option<&Entity<ColumnSettingsView>> {
        self.column_settings.as_ref()
    }

    /// Shows the settings dialog of column `id` (in the active deck).
    pub fn open_column_settings(
        &mut self,
        id: ColumnId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self.deck.active().columns.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        self.close_overlays(cx);
        let viewer = self.viewer().cloned().map(Arc::new);
        let view = cx.new(|cx| {
            ColumnSettingsView::new(id, &config.source, viewer, config.settings, window, cx)
        });
        self._column_settings_sub =
            Some(cx.subscribe(&view, |this, _, e: &ColumnSettingsEvent, cx| match e {
                ColumnSettingsEvent::Preview(id, settings) => {
                    this.set_column_settings(*id, settings.clone(), cx);
                }
                ColumnSettingsEvent::Changed(id, settings) => {
                    this.set_column_settings(*id, settings.clone(), cx);
                    this.persist(cx);
                }
                ColumnSettingsEvent::Dismissed => this.close_overlays(cx),
            }));
        self.column_settings = Some(view);
        cx.notify();
    }

    /// Applies edited settings to column `id` and its view (without saving).
    fn set_column_settings(
        &mut self,
        id: ColumnId,
        settings: ColumnSettings,
        cx: &mut Context<Self>,
    ) {
        if let Some(config) = self.deck.column_mut(id) {
            config.settings = settings.clone();
        }
        if let Some((_, view)) = self.columns.iter().find(|(cid, _)| *cid == id) {
            view.update(cx, |c, cx| c.set_settings(settings, cx));
        }
        cx.notify();
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
        self.settings_scroll = ScrollHandle::new();
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

    /// Turns automatic checks for new tweets on or off, and saves the choice.
    pub fn set_polling(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.polling = Some(enabled);
        self.update_polling(cx);
        self.save_settings(cx);
    }

    /// Turns the UI's animations on or off (GPUI's reduce motion).
    pub fn set_animations(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.animations = Some(enabled);
        cx.set_reduce_motion(!enabled);
        self.save_settings(cx);
    }

    /// Whether timeline GIFs start on their own when on screen.
    pub fn set_gif_autoplay(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.gif_autoplay = Some(enabled);
        cx.set_global(GifAutoplay(self.settings.gif_autoplay()));
        self.save_settings(cx);
    }

    /// Caps the height of tweets' media (`None`: no cap). Rows already
    /// measured are measured again, as their height changes. Saved when
    /// `save` (the slider was released), else only shown.
    pub fn set_media_max_height(
        &mut self,
        limit: Option<MaxHeight>,
        save: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.settings.media_max_height != limit;
        if changed {
            self.settings.media_max_height = limit;
            cx.set_global(MediaMaxHeight(limit));
            for (_, column) in &self.columns {
                column.update(cx, |c, cx| c.remeasure(cx));
            }
            // Every sheet, the lower ones too: they redraw only when notified.
            for open in self.sheets.iter() {
                match &open.sheet {
                    Sheet::Profile(panel) => panel.update(cx, |p, cx| p.remeasure(cx)),
                    Sheet::Thread(panel) => panel.update(cx, |_, cx| cx.notify()),
                }
            }
        }
        if save {
            self.save_settings(cx);
        } else if changed {
            cx.notify();
        }
    }

    /// Turns the periodic update check on or off.
    pub fn set_check_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.check_updates = Some(enabled);
        self.schedule_update_checks(cx);
        self.save_settings(cx);
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.options.settings_path.clone() {
            let settings = self.settings.clone();
            let previous = self.settings_save.take();
            self.settings_save = Some(cx.background_spawn(async move {
                if let Some(previous) = previous {
                    previous.await;
                }
                if let Err(e) = settings.save(&path) {
                    tracing::error!("saving settings to {} failed: {e}", path.display());
                }
            }));
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
                .text_button(Size::Small)
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
            .track_scroll(&self.settings_scroll)
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
                    .label("Check for new tweets automatically (within Twitter's rate limits)")
                    .on_click({
                        let weak = weak.clone();
                        move |checked, _, cx| {
                            weak.update(cx, |this, cx| this.set_polling(*checked, cx)).ok();
                        }
                    }),
            )
            .child(
                Checkbox::new("gif-autoplay")
                    .checked(self.settings.gif_autoplay() == Autoplay::On)
                    .label("Play GIFs automatically when they are on screen")
                    .on_click({
                        let weak = weak.clone();
                        move |checked, _, cx| {
                            weak.update(cx, |this, cx| this.set_gif_autoplay(*checked, cx)).ok();
                        }
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .text_sm()
                            .child("Maximum media height (taller media are cropped)")
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(MaxHeight::label(self.settings.media_max_height)),
                            ),
                    )
                    .child(
                        Slider::new(&self.media_height).accessibility_label("Maximum media height"),
                    ),
            )
            .child(section("Appearance"))
            .child(
                Checkbox::new("animations")
                    .checked(self.settings.animations_enabled())
                    .label("Animations (off also stills loading spinners and placeholders)")
                    .on_click(move |checked, _, cx| {
                        weak.update(cx, |this, cx| this.set_animations(*checked, cx)).ok();
                    }),
            )
            .child(section("Updates"))
            .child(self.render_update_settings(cx))
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
                                .text_button(Size::XSmall)
                                .label("Show")
                                .on_click(move |_, _, cx| cx.reveal_path(&log)),
                        ),
                )
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("settings-close")
                        .text_button(Size::Medium)
                        .label("Done")
                        .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                ),
            )
            .into_any_element()
    }

    /// The version, and (for an installed copy) the update setting and "Check now".
    fn render_update_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let version = h_flex()
            .gap_2()
            .items_center()
            .text_sm()
            .child(div().flex_1().child(format!("Twitty {}", self.updates.current())));
        if self.options.updates.is_none() {
            return v_flex()
                .gap_1()
                .child(version)
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    "This copy wasn't installed with Twitty's installer, so it doesn't update itself.",
                ))
                .into_any_element();
        }
        let weak = cx.entity().downgrade();
        v_flex()
            .gap_2()
            .child(
                version.child(
                    Button::new("update-check")
                        .text_button(Size::XSmall)
                        .label("Check now")
                        .on_click(cx.listener(|this, _, _, cx| this.check_for_updates(cx))),
                ),
            )
            .child(
                Checkbox::new("check-updates")
                    .checked(self.settings.check_updates())
                    .label("Look for new versions on GitHub")
                    .on_click(move |checked, _, cx| {
                        weak.update(cx, |this, cx| this.set_check_updates(*checked, cx)).ok();
                    }),
            )
            .into_any_element()
    }

    /// One line per request type: responses, rows, rows Twitty couldn't read,
    /// errors, and the last problem. Unreadable rows mean Twitter changed its data.
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

    fn render_account_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.options.account.as_ref()?;
        if self.sidebar_expanded() {
            return Some(self.render_account_row(cx));
        }
        let button = Button::new("account")
            .ghost()
            .large()
            .w(px(52.))
            .h(px(52.))
            .debug_selector(|| "account-button".into());
        Some(if self.options.session.is_some() {
            let tip = match &self.viewer {
                Some(v) => format!("Signed in as @{}", v.handle),
                None => "Account".to_owned(),
            };
            let face = self.viewer.as_ref().and_then(|v| v.avatar.as_ref());
            match face {
                Some(src) => button.child(
                    img(crate::ui::remote_image::source(
                        src.url.as_str(),
                        ACCOUNT_AVATAR,
                        ACCOUNT_AVATAR,
                    ))
                    .size(px(ACCOUNT_AVATAR))
                    // The button's padding would squeeze it into an oval.
                    .flex_none()
                    .aspect_ratio(1.)
                    .object_fit(ObjectFit::Cover)
                    .rounded_full()
                    .debug_selector(|| "account-avatar".into()),
                ),
                None => button.icon(Icon::new(IconName::CircleUser).size(px(26.))),
            }
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

    /// The account entry of the expanded sidebar: the avatar with the
    /// viewer's name and `@handle` beside it, or a sign-in row.
    fn render_account_row(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.options.session.is_none() {
            let entry = RailEntry {
                id: "account".into(),
                selector: "account-button".into(),
                icon: IconName::LogIn.path(),
                label: "Sign in".into(),
                selected: false,
            };
            return self.render_rail_entry(
                entry,
                None,
                |this, window, cx| this.open_login(None, window, cx),
                cx,
            );
        }
        let theme = cx.theme();
        let face: AnyElement = match self.viewer.as_ref().and_then(|v| v.avatar.as_ref()) {
            Some(src) => img(crate::ui::remote_image::source(
                src.url.as_str(),
                ACCOUNT_AVATAR,
                ACCOUNT_AVATAR,
            ))
            .size(px(ACCOUNT_AVATAR))
            .flex_none()
            .aspect_ratio(1.)
            .object_fit(ObjectFit::Cover)
            .rounded_full()
            .debug_selector(|| "account-avatar".into())
            .into_any_element(),
            None => div()
                .flex_none()
                .size(px(ACCOUNT_AVATAR))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::CircleUser).size(px(26.)))
                .into_any_element(),
        };
        let names = match &self.viewer {
            Some(v) => v_flex()
                .flex_1()
                .min_w_0()
                // The default line height spreads the two lines too far apart.
                .line_height(gpui_kit::relative(1.2))
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .debug_selector(|| "account-name".into())
                        .child(emoji::text(v.display_name.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .debug_selector(|| "account-handle".into())
                        .child(format!("@{}", v.handle)),
                ),
            None => v_flex().flex_1().min_w_0().child(div().truncate().child("Account")),
        };
        h_flex()
            .id("account")
            .debug_selector(|| "account-button".into())
            .w_full()
            .h(px(52.))
            // Centres the avatar on the entries' icons above it.
            .pl(px(11.))
            .pr_2()
            .gap_3()
            .items_center()
            .rounded_lg()
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(face)
            .child(names)
            .on_click(cx.listener(|this, _, _, cx| {
                this.account_panel = !this.account_panel;
                cx.notify();
            }))
            .into_any_element()
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
                            .text_button(Size::Medium)
                            .label("Close")
                            .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                    )
                    .when(can_sign_out, |d| {
                        d.child(
                            Button::new("sign-out")
                                .danger()
                                .icon(IconName::LogOut)
                                .text_button(Size::Medium)
                                .label("Sign out")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.sign_out(window, cx)),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// One sidebar entry: an icon button with its label as tooltip, or, in
    /// the expanded sidebar, a row with the icon, the label and `trailing`.
    fn render_rail_entry(
        &self,
        entry: RailEntry,
        trailing: Option<AnyElement>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RailEntry { id, selector, icon, label, selected } = entry;
        let on_click = cx.listener(move |this, _, window, cx| on_click(this, window, cx));
        if !self.sidebar_expanded() {
            return Button::new(id)
                .debug_selector(move || selector)
                .ghost()
                .large()
                .icon(Icon::empty().path(icon).size(px(24.)))
                .tooltip(label)
                .selected(selected)
                .w(px(52.))
                .h(px(RAIL_ENTRY_HEIGHT))
                .on_click(on_click)
                .into_any_element();
        }
        let theme = cx.theme();
        h_flex()
            .id(id)
            .debug_selector(move || selector)
            .w_full()
            .h(px(RAIL_ENTRY_HEIGHT))
            .pl(px(14.))
            .pr_2()
            .gap_3()
            .items_center()
            .rounded_lg()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.list_active).font_weight(FontWeight::BOLD))
            .hover(|s| s.bg(theme.list_hover))
            .child(Icon::empty().path(icon).size(px(24.)))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .children(trailing)
            .on_click(on_click)
            .into_any_element()
    }

    /// One deck in the sidebar: a row spanning the whole rail, so the
    /// selected deck's background reaches the window's edge, where a
    /// primary-coloured bar marks it.
    fn render_deck_entry(
        &self,
        entry: RailEntry,
        trailing: Option<AnyElement>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RailEntry { id, selector, icon, label, selected } = entry;
        let theme = cx.theme();
        let row = h_flex()
            .id(id)
            .debug_selector(move || selector)
            .w_full()
            .h(px(RAIL_ENTRY_HEIGHT))
            .items_center()
            .cursor_pointer()
            // Always there, so selecting a deck doesn't shift its icon.
            .border_l(px(DECK_MARK_WIDTH))
            .border_color(if selected { theme.primary } else { gpui_kit::transparent_black() })
            .when(selected, |d| d.bg(theme.list_active).font_weight(FontWeight::BOLD))
            .hover(|s| s.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)));
        if self.sidebar_expanded() {
            // Lines the icon up with the padded entries above and below.
            row.pl(px(RAIL_PADDING + 14. - DECK_MARK_WIDTH))
                .pr(px(RAIL_PADDING + 8.))
                .gap_3()
                .child(Icon::empty().path(icon).size(px(24.)))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .children(trailing)
                .into_any_element()
        } else {
            // Balances the bar so the icon stays centred.
            row.pr(px(DECK_MARK_WIDTH))
                .justify_center()
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                .child(Icon::empty().path(icon).size(px(24.)))
                .into_any_element()
        }
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let expanded = self.sidebar_expanded();
        let action = |name: &'static str, icon: SharedString, label: &'static str| RailEntry {
            id: name.into(),
            selector: format!("rail-{name}"),
            icon,
            label: label.into(),
            selected: false,
        };
        let compose =
            Button::new("compose")
                .debug_selector(|| "rail-compose".into())
                .primary()
                .icon(Icon::new(IconName::PenLine).size(px(22.)))
                .rounded_full()
                .h(px(52.))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_compose(ComposeTarget::New, window, cx)
                }));
        let compose = if expanded {
            compose.text_button(Size::Large).label("Tweet").w_full().h(px(48.))
        } else {
            compose.tooltip("Tweet").w(px(52.))
        };
        let active = self.deck.active_index();
        let decks: Vec<_> = self
            .deck
            .decks
            .iter()
            .enumerate()
            .map(|(ix, deck)| {
                let entry = RailEntry {
                    id: ("rail-deck", ix).into(),
                    selector: format!("rail-deck-{ix}"),
                    icon: deck_settings::icon(deck.icon).path(),
                    label: deck.name.clone().into(),
                    selected: ix == active,
                };
                let edit = expanded.then(|| {
                    Button::new(("deck-edit", ix))
                        .debug_selector(move || format!("deck-edit-{ix}"))
                        .ghost()
                        .small()
                        .icon(IconName::Pencil)
                        .tooltip("Edit deck")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // Editing a deck doesn't switch to it.
                            cx.stop_propagation();
                            this.open_deck_settings(Some(ix), window, cx);
                        }))
                        .into_any_element()
                });
                self.render_deck_entry(entry, edit, move |this, _, cx| this.switch_deck(ix, cx), cx)
            })
            .collect();
        let toggle = if expanded {
            action("toggle", IconName::PanelLeftClose.path(), "Collapse menu")
        } else {
            action("toggle", IconName::PanelLeftOpen.path(), "Expand menu")
        };
        // The decks span the whole rail; everything else is inset.
        let inset = |d: gpui_kit::Div| {
            let d = d.flex_none().gap_1();
            if expanded { d.px(px(RAIL_PADDING)) } else { d.items_center() }
        };
        let top = inset(v_flex())
            .child(compose)
            .child(self.render_rail_entry(
                action("search-people", IconName::Search.path(), "Search people"),
                None,
                |this, window, cx| this.open_user_search(window, cx),
                cx,
            ))
            .child(self.render_rail_entry(
                action("add-column", ADD_COLUMN.into(), "Add column"),
                None,
                |this, window, cx| this.open_add_column(window, cx),
                cx,
            ))
            .child(
                div()
                    .flex_none()
                    .h(px(1.))
                    .my_1()
                    .map(|d| if expanded { d.w_full() } else { d.w(px(36.)) })
                    .bg(theme.border),
            )
            .child(
                div()
                    .pt_1()
                    .text_color(theme.muted_foreground)
                    .debug_selector(|| "rail-decks-heading".into())
                    // Smaller when collapsed so it fits the narrow rail.
                    .map(|d| if expanded { d.pl(px(14.)).text_sm() } else { d.text_xs() })
                    .child("Decks"),
            )
            .child(self.render_rail_entry(
                action("deck-add", ADD_DECK.into(), "New deck"),
                None,
                |this, window, cx| this.open_deck_settings(None, window, cx),
                cx,
            ));
        let bottom = inset(v_flex())
            .children(self.render_account_button(cx))
            .child(self.render_rail_entry(
                action("settings", IconName::Settings.path(), "Settings"),
                None,
                |this, _, cx| this.open_settings(cx),
                cx,
            ))
            .child(self.render_rail_entry(
                toggle,
                None,
                move |this, _, cx| this.set_sidebar_expanded(!expanded, cx),
                cx,
            ));
        v_flex()
            .flex_none()
            .w(px(if expanded { RAIL_EXPANDED_WIDTH } else { RAIL_WIDTH }))
            .h_full()
            .py_2()
            .gap_1()
            .child(top)
            // One entry per deck; clicking one switches to it. Scrolls itself
            // when there are more decks than fit.
            .child(
                v_flex()
                    .id("rail-decks")
                    .w_full()
                    .min_h_0()
                    .gap_1()
                    .overflow_y_scroll()
                    .children(decks),
            )
            .child(div().flex_1())
            .child(bottom)
    }
}

/// What a sidebar entry shows (see `TwittyApp::render_rail_entry`).
struct RailEntry {
    id: ElementId,
    /// For tests (`debug_bounds`).
    selector: String,
    /// The icon's asset path (`IconName::path` or one of `assets`' own).
    icon: SharedString,
    label: SharedString,
    selected: bool,
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
            // The overflow is only known once the deck is laid out: check it
            // right after, and render again if the strip must appear or go.
            let deck_scroll = self.deck_scroll.clone();
            let rendered_overflows = self.deck_overflows;
            let this = cx.entity().downgrade();
            let overflow_check = canvas(
                move |_, _, cx| {
                    let overflows = deck_scroll.max_offset().x > px(0.);
                    if overflows != rendered_overflows {
                        this.update(cx, |this, cx| {
                            this.deck_overflows = overflows;
                            cx.notify();
                        })
                        .ok();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute();
            // The scrollbar is an overlay on this wrapper (not on the scrolling
            // element, whose children move with the scroll). While it shows, the
            // bottom padding gives it its own strip so it never covers the
            // columns; when they all fit it is hidden, and they reach the bottom.
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .when(self.deck_overflows, |d| d.pb(px(DECK_SCROLLBAR_GUTTER)))
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
                .child(overflow_check)
                .horizontal_scrollbar(&self.deck_scroll)
                .into_any_element()
        };
        let backdrop = theme.overlay.alpha(BACKDROP_ALPHA);
        // (name for the enter animation, whether it is a card, content)
        let overlay = if let Some((post, index)) = &self.media {
            Some(("media", false, self.render_media_viewer(post, *index, true, window, cx)))
        } else if let Some(login) = &self.login {
            Some(("login", true, login.clone().into_any_element()))
        } else if let Some(add) = &self.add_column {
            Some(("add-column", true, add.clone().into_any_element()))
        } else if let Some(view) = &self.column_settings {
            Some(("column-settings", true, view.clone().into_any_element()))
        } else if let Some(view) = &self.deck_settings {
            Some(("deck-settings", true, view.clone().into_any_element()))
        } else if let Some(compose) = &self.compose {
            Some(("compose", true, compose.clone().into_any_element()))
        } else if let Some(view) = &self.repost {
            Some(("repost", true, view.clone().into_any_element()))
        } else if let Some(view) = &self.user_search {
            Some(("user-search", true, view.clone().into_any_element()))
        } else if self.account_panel {
            Some(("account", true, self.render_account_panel(cx)))
        } else if self.settings_panel {
            Some(("settings", true, self.render_settings_panel(cx)))
        } else {
            None
        };
        let has_overlay = overlay.is_some();
        let sheet_open = !self.sheets.is_empty();
        let leaving = if has_overlay { None } else { self.render_leaving(backdrop, window, cx) };
        // The sheet's backdrop fades in when it opens, not each time
        // an overlay over it closes.
        let sheet_fresh =
            self.sheet_opened.is_some_and(|at| at.elapsed() < theme.motion.duration_normal);
        let tokens = theme.motion.clone();
        let enter = move || {
            let easing = tokens.easing_enter.clone();
            Animation::new(tokens.duration_normal)
                .with_easing(move |t| easing.sample(t).clamp(0., 1.))
        };
        // Sheets close slower than dialogs, slowing down as they go, so a
        // 600px panel doesn't vanish at full speed. The sheet under one that
        // closes glides back in step with it.
        let sheet_close = {
            let tokens = theme.motion.clone();
            move || {
                let easing = tokens.easing_enter.clone();
                Animation::new(tokens.duration_slow)
                    .with_easing(move |t| easing.sample(t).clamp(0., 1.))
            }
        };
        let sheet_layers = self.render_sheets(enter.clone(), sheet_close.clone(), cx);
        let leaving_sheets = self.render_leaving_sheets(sheet_close.clone());
        let demo = self.options.session.is_none();
        let can_sign_in = self.options.account.is_some();
        // At the top of the deck area, so the rail and the title bar still meet.
        let banner =
            demo.then(|| {
                h_flex()
                    .w_full()
                    .justify_center()
                    .items_center()
                    .gap_3()
                    .py_1()
                    .text_xs()
                    .rounded_tl(px(CHROME_RADIUS - 1.))
                    .bg(theme.primary.opacity(0.15))
                    .text_color(theme.primary)
                    .child("Demo mode: showing generated tweets")
                    .when(can_sign_in, |d| {
                        d.child(
                            Button::new("banner-sign-in")
                                .text_button(Size::XSmall)
                                .label("Sign in")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_login(None, window, cx)
                                })),
                        )
                    })
            });
        let update_banner = self.render_update_banner(!demo, cx);
        // Overlays cover this part only, so the title bar always stays usable.
        let content = v_flex()
            .debug_selector(|| "app-content".into())
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
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
                    .on_action(cx.listener(|this, _: &keys::ColumnLeft, window, cx| {
                        this.step_left_right(-1, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::ColumnRight, window, cx| {
                        this.step_left_right(1, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::LikeSelected, _, cx| {
                        this.toggle_selected(Toggle::Like, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::RepostSelected, window, cx| {
                        this.repost_selected(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::BookmarkSelected, _, cx| {
                        this.toggle_selected(Toggle::Bookmark, cx)
                    }))
                    .on_action(cx.listener(|this, _: &keys::ReplySelected, window, cx| {
                        if let Some(post) = this.selected_post(cx) {
                            this.open_compose(ComposeTarget::Reply(post), window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::OpenSelected, _, cx| {
                        if let Some(post) = this.selected_post(cx) {
                            this.open_thread(post.id.clone(), cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::NewPost, window, cx| {
                        if this.media.is_none() {
                            this.open_compose(ComposeTarget::New, window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &keys::Dismiss, _, cx| this.dismiss(cx)))
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .child(
                        // One border along the title bar and the rail, rounded
                        // where they meet.
                        v_flex()
                            .debug_selector(|| "deck-area".into())
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_t_1()
                            .border_l_1()
                            .border_color(theme.border)
                            .rounded_tl(px(CHROME_RADIUS))
                            .children(banner)
                            .children(update_banner)
                            .child(div().flex_1().min_h_0().flex().child(body)),
                    )
                    // The sheets over the deck's right side. Their backdrop
                    // takes the whole click that closes them, so nothing under
                    // it (a tweet, an image, a link) reacts. With an overlay
                    // open, that overlay's backdrop darkens everything.
                    .when(sheet_open && !has_overlay, |d| {
                        let shade = div()
                            .id("thread-backdrop")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .occlude()
                            .bg(backdrop)
                            .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_top_sheet(cx);
                            }));
                        if sheet_fresh {
                            d.child(
                                shade.with_animation("thread-backdrop-in", enter(), |d, t| {
                                    d.opacity(t)
                                }),
                            )
                        } else {
                            d.child(shade)
                        }
                    })
                    .children(sheet_layers)
                    // Sheets that closed, sliding out: no handlers, and a cover
                    // so nothing inside them reacts.
                    .when(!sheet_open && !has_overlay && !leaving_sheets.is_empty(), |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .bg(backdrop)
                                .with_animation("thread-backdrop-out", sheet_close(), |d, t| {
                                    d.opacity(1. - t)
                                }),
                        )
                    })
                    .children(leaving_sheets),
            )
            .when_some(overlay, |d, (name, card, overlay)| {
                // The image viewer fills the window, so it only fades; cards
                // also rise a little into place.
                let content = if card {
                    // Occluding, so only clicks beside the card reach the
                    // backdrop under it.
                    div()
                        .debug_selector(|| "overlay-card".into())
                        .relative()
                        .occlude()
                        .child(overlay)
                        .with_animation(anim_id("overlay-in", name), enter(), |d, t| {
                            d.opacity(t).top(px(motion::TRAVEL * (1. - t)))
                        })
                        .into_any_element()
                } else {
                    div()
                        .size_full()
                        .child(overlay)
                        .with_animation(anim_id("overlay-in", name), enter(), |d, t| d.opacity(t))
                        .into_any_element()
                };
                // Over the thread panel, the panel's own backdrop was already
                // drawn: fading this one in would flash lighter.
                let shade = div()
                    .debug_selector(|| "overlay-backdrop".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(backdrop)
                    // A press beside a dialog closes it, like Esc. The image
                    // viewer handles its own clicks.
                    .when(card, |d| {
                        d.on_any_mouse_down(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss(cx);
                        }))
                    });
                let shade = if sheet_open {
                    shade.into_any_element()
                } else {
                    shade
                        .with_animation(anim_id("backdrop-in", name), enter(), |d, t| d.opacity(t))
                        .into_any_element()
                };
                d.child(
                    div()
                        .key_context(keys::OVERLAY)
                        .on_action(cx.listener(|this, _: &keys::Dismiss, _, cx| this.dismiss(cx)))
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .child(shade)
                        .child(centered().child(content)),
                )
            })
            .children(leaving);
        let toasts = toast_view::render(
            &self.toasts,
            cx.background_executor().now(),
            &theme,
            cx.reduce_motion(),
            cx.listener(|this, id: &u64, _, cx| this.dismiss_toast(*id, cx)),
        );
        v_flex()
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(render_title_bar(&theme))
            // Over the overlays too, so a result shows whatever is open.
            .child(content.children(toasts))
            .child(self.autoscroll.clone())
    }
}

/// The window's title bar, in place of the system one: the app's background
/// colour, the logo and name centred, and the native minimise / maximise /
/// close buttons on the right (on Windows they are real caption buttons, so
/// Snap layouts work). Dragging it moves the window. It has no border of its
/// own: the deck's top border runs under it (see `CHROME_RADIUS`).
fn render_title_bar(theme: &gpui_kit::component::Theme) -> impl IntoElement {
    // Centred on the whole window, not on the space left of the buttons. The
    // layer has no handlers, so drags and clicks reach the bar under it.
    // White on dark themes; muted on light ones, where black would be loud.
    let name_color = if theme.is_dark() { theme.foreground } else { theme.muted_foreground };
    let title = h_flex()
        .debug_selector(|| "window-title".into())
        .gap_1p5()
        .items_center()
        .child({
            use crate::ui::assets::{LOGO, LOGO_SIZE};
            img(crate::ui::remote_image::asset(LOGO, LOGO_SIZE, LOGO_SIZE)).size(px(LOGO_SIZE))
        })
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(name_color)
                .child("Twitty"),
        );
    div()
        .relative()
        .flex_none()
        .child(title_bar::TitleBar::new(theme.background))
        .child(centered().child(title))
}

/// Id of the animation `what` of the overlay `name` (e.g. "overlay-in", "compose").
fn anim_id(what: &str, name: &str) -> gpui_kit::ElementId {
    gpui_kit::ElementId::Name(format!("{what}-{name}").into())
}

/// A full-window layer that centres its child.
fn centered() -> gpui_kit::Div {
    div().absolute().top_0().left_0().size_full().flex().items_center().justify_center()
}

/// `el` (which must be positioned) with a transparent cover on top that
/// takes the mouse, so a view that is only animating out can't be clicked.
fn inert(el: gpui_kit::Div) -> gpui_kit::Div {
    el.child(div().absolute().top_0().left_0().size_full().occlude())
}

impl TwittyApp {
    /// The open sheets, the deepest first. The top one slides in a short way
    /// from the right edge when it opens, so its hitbox is about where it
    /// ends up from the first frame; the ones under it slide to their place,
    /// a little further left at each level. A press on a lower sheet's edge
    /// closes the top one, like one on the backdrop, and nothing inside it
    /// reacts.
    fn render_sheets(
        &self,
        enter: impl Fn() -> Animation,
        close: impl Fn() -> Animation,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let shade = cx.theme().overlay.alpha(LOWER_SHEET_ALPHA);
        let closing = matches!(self.sheets.last_change(), sheets::Change::Popped(_));
        self.sheets
            .visible()
            .map(|(depth, open)| {
                let to = sheets::offset(depth);
                let from = self.sheets.moved_from(depth).map(sheets::offset);
                let layer = div()
                    .debug_selector(move || format!("sheet-{depth}"))
                    .absolute()
                    .top_0()
                    .right(px(to))
                    .h_full()
                    .occlude()
                    // A scroll in the top sheet redraws its ancestors, so every
                    // sheet would be laid out again each frame. Lower sheets
                    // take no input: they reuse their last frame until they
                    // change themselves.
                    .child(if depth == 0 {
                        open.sheet.view().into_any_element()
                    } else {
                        open.sheet
                            .view()
                            .cached(
                                gpui_kit::StyleRefinement::default()
                                    .w(px(crate::ui::thread::PANEL_WIDTH))
                                    .h_full(),
                            )
                            .into_any_element()
                    })
                    .when(depth > 0, |d| {
                        d.child(
                            div()
                                .debug_selector(move || format!("sheet-edge-{depth}"))
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .occlude()
                                .bg(shade)
                                .on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_top_sheet(cx);
                                })),
                        )
                    });
                // A new id at each level, so a sheet animates each time it moves.
                let id = ElementId::Name(
                    format!("sheet-{}-{depth}", open.sheet.entity_id().as_u64()).into(),
                );
                match from {
                    None => layer
                        .with_animation(id, enter(), move |d, t| {
                            d.right(px(to - motion::PANEL_TRAVEL * (1. - t))).opacity(t)
                        })
                        .into_any_element(),
                    Some(from) => layer
                        .with_animation(id, if closing { close() } else { enter() }, move |d, t| {
                            d.right(px(from + (to - from) * t))
                        })
                        .into_any_element(),
                }
            })
            .collect()
    }

    /// The sheets that just closed, sliding out from where they were.
    fn render_leaving_sheets(&self, close: impl Fn() -> Animation) -> Vec<AnyElement> {
        let Some(leaving) = &self.leaving_sheet else {
            return Vec::new();
        };
        leaving
            .what
            .iter()
            .map(|(sheet, depth)| {
                let at = sheets::offset(*depth);
                let id =
                    ElementId::Name(format!("sheet-out-{}", sheet.entity_id().as_u64()).into());
                inert(div().absolute().top_0().right(px(at)).h_full().child(sheet.view()))
                    .with_animation(id, close(), move |d, t| {
                        d.right(px(at - motion::PANEL_TRAVEL * t)).opacity(1. - t)
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// The overlay that just closed, fading out. It has no key context, no
    /// handlers and no occluding backdrop: clicks go to what's under it,
    /// except on its card, which is covered.
    fn render_leaving(
        &self,
        backdrop: gpui_kit::Hsla,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let leaving = self.leaving.as_ref()?;
        let (name, content) = match &leaving.what {
            LeavingOverlay::Media(post, index) => {
                ("media", self.render_media_viewer(post, *index, false, window, cx))
            }
            LeavingOverlay::View(view) => ("view", view.clone().into_any_element()),
            LeavingOverlay::Account => ("account", self.render_account_panel(cx)),
            LeavingOverlay::Settings => ("settings", self.render_settings_panel(cx)),
        };
        let media = matches!(leaving.what, LeavingOverlay::Media(..));
        let tokens = cx.theme().motion.clone();
        let exit = move || {
            let easing = tokens.easing_exit.clone();
            Animation::new(tokens.duration_fast)
                .with_easing(move |t| easing.sample(t).clamp(0., 1.))
        };
        let content = if media {
            // Rendered without handlers already.
            div()
                .size_full()
                .child(content)
                .with_animation(anim_id("overlay-out", name), exit(), |d, t| d.opacity(1. - t))
                .into_any_element()
        } else {
            inert(div().relative().child(content))
                .with_animation(anim_id("overlay-out", name), exit(), |d, t| {
                    d.opacity(1. - t).top(px(motion::TRAVEL * t))
                })
                .into_any_element()
        };
        // Over the thread panel, its own backdrop is back already.
        let shade = self.sheets.is_empty().then(|| {
            div().absolute().top_0().left_0().size_full().bg(backdrop).with_animation(
                anim_id("backdrop-out", name),
                exit(),
                |d, t| d.opacity(1. - t),
            )
        });
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(shade)
                .child(centered().child(content))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{ConnectError, CredentialStore, Credentials, MemoryStore};
    use gpui_kit::{
        Modifiers, MouseButton, Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext,
        VisualTestContext, point,
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

    /// A deck wide enough to scroll sideways, its columns full of tweets.
    fn autoscroll_deck(cx: &mut TestAppContext) -> (Entity<TwittyApp>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::runtime::Runtime::inline());
            keys::bind(cx);
        });
        let mut deck = DeckFile::default();
        for _ in 0..8 {
            deck.add_column(FeedSource::Home { mode: twitty_model::HomeMode::Following });
        }
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, AppOptions::default(), window, cx)
        });
        cx.run_until_parked();
        (app, cx)
    }

    /// (first column's scroll position, deck's horizontal offset)
    fn autoscroll_positions(
        app: &Entity<TwittyApp>,
        cx: &mut VisualTestContext,
    ) -> ((usize, Pixels), Pixels) {
        app.read_with(cx, |app, cx| {
            (app.columns[0].1.read(cx).scroll_top(), app.deck_scroll.offset().x)
        })
    }

    fn autoscrolling(app: &Entity<TwittyApp>, cx: &mut VisualTestContext) -> bool {
        app.read_with(cx, |app, cx| app.autoscroll.read(cx).is_active())
    }

    #[gpui_kit::test]
    fn middle_drag_scrolls_towards_the_pointer_until_released(cx: &mut TestAppContext) {
        let (app, cx) = autoscroll_deck(cx);
        let anchor = point(px(250.), px(400.));
        let start = autoscroll_positions(&app, cx);
        assert_eq!(start, ((0, px(0.)), px(0.)));

        cx.simulate_mouse_down(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_move(
            anchor + point(px(0.), px(200.)),
            MouseButton::Middle,
            Modifiers::default(),
        );
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        let (top, deck_x) = autoscroll_positions(&app, cx);
        assert!(top.0 > 0, "the column under the anchor scrolled down");
        assert_eq!(deck_x, px(0.), "a vertical move doesn't scroll sideways");

        // Diagonally: the deck scrolls right, and the column pressed on keeps
        // scrolling down (like Chrome, the target is kept), even once the
        // deck slid it out of view. The columns that slide under the anchor
        // don't move.
        cx.simulate_mouse_move(
            anchor + point(px(200.), px(200.)),
            MouseButton::Middle,
            Modifiers::default(),
        );
        let column_width =
            app.read_with(cx, |app, cx| app.columns[0].1.read(cx).settings().width.px());
        let mut last = top;
        for _ in 0..3 {
            cx.executor().advance_clock(std::time::Duration::from_millis(300));
            cx.run_until_parked();
            let (later, _) = autoscroll_positions(&app, cx);
            assert!(later > last, "the pressed column keeps scrolling down");
            last = later;
        }
        let deck_x = autoscroll_positions(&app, cx).1;
        assert!(deck_x < -px(column_width), "the first column is out of view");
        let others = app.read_with(cx, |app, cx| {
            app.columns[1..].iter().map(|(_, c)| c.read(cx).scroll_top()).collect::<Vec<_>>()
        });
        assert!(others.iter().all(|top| *top == (0, px(0.))), "{others:?}");

        cx.simulate_mouse_up(
            anchor + point(px(200.), px(200.)),
            MouseButton::Middle,
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert!(!autoscrolling(&app, cx), "a drag stops on release");
        let stopped = autoscroll_positions(&app, cx);
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(autoscroll_positions(&app, cx), stopped);
    }

    #[gpui_kit::test]
    fn a_middle_click_scrolls_until_the_next_press(cx: &mut TestAppContext) {
        let (app, cx) = autoscroll_deck(cx);
        let anchor = point(px(250.), px(400.));
        cx.simulate_mouse_down(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_up(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_move(anchor + point(px(0.), px(150.)), None, Modifiers::default());
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        assert!(autoscroll_positions(&app, cx).0.0 > 0, "scrolls with the button up");

        // The press that stops it does nothing else: no conversation opens.
        let tweet = anchor + point(px(0.), px(150.));
        cx.simulate_click(tweet, Modifiers::default());
        cx.run_until_parked();
        assert!(!autoscrolling(&app, cx));
        assert!(app.read_with(cx, |app, _| app.sheets.is_empty()));
        let stopped = autoscroll_positions(&app, cx);
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(autoscroll_positions(&app, cx), stopped);

        // Esc stops it too.
        cx.simulate_mouse_down(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_up(anchor, MouseButton::Middle, Modifiers::default());
        assert!(autoscrolling(&app, cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!autoscrolling(&app, cx));

        // And so does the user's own wheel (but not the wheel events it sends).
        cx.simulate_mouse_down(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_up(anchor, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_move(anchor + point(px(0.), px(150.)), None, Modifiers::default());
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        assert!(autoscrolling(&app, cx), "its own wheel events don't stop it");
        cx.simulate_event(ScrollWheelEvent {
            position: anchor,
            delta: ScrollDelta::Lines(point(0., -1.)),
            ..Default::default()
        });
        cx.run_until_parked();
        assert!(!autoscrolling(&app, cx));
        let stopped = autoscroll_positions(&app, cx);
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(autoscroll_positions(&app, cx), stopped);
    }

    #[gpui_kit::test]
    fn a_middle_press_beside_a_sheet_does_not_close_it(cx: &mut TestAppContext) {
        let (app, cx) = autoscroll_deck(cx);
        let post = app.read_with(cx, |app, cx| {
            app.columns[0].1.read(cx).visible_items().iter().find_map(|item| match item {
                twitty_model::FeedItem::Post(post) => Some(post.id.clone()),
                _ => None,
            })
        });
        let post = post.expect("the mock home timeline has tweets");
        app.update(cx, |app, cx| app.open_thread(post, cx));
        cx.run_until_parked();
        // The backdrop beside the sheet closes it on any other press.
        cx.simulate_mouse_down(
            point(px(250.), px(400.)),
            MouseButton::Middle,
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert!(autoscrolling(&app, cx));
        assert!(!app.read_with(cx, |app, _| app.sheets.is_empty()));
    }

    #[gpui_kit::test]
    fn the_app_draws_its_own_title_bar_above_dialogs(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::runtime::Runtime::inline());
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
        // A dialog's backdrop must not cover the title bar.
        app.update(cx, |app, cx| app.open_settings(cx));
        cx.run_until_parked();
        let content = cx.debug_bounds("app-content").expect("content is drawn");
        let title = cx.debug_bounds("window-title").expect("the title is drawn");
        let deck = cx.debug_bounds("deck-area").expect("the deck is drawn");
        cx.update(|window, _| {
            let bar = window.find("title-bar");
            assert!(bar.visible());
            assert_eq!(bar.bounds().top(), px(0.), "the bar is at the very top");
            assert_eq!(bar.bounds().size.height, px(39.));
            assert!(content.top() >= bar.bounds().bottom(), "dialogs live below the bar");
            let middle = window.viewport_size().width / 2.;
            assert!((title.center().x - middle).abs() < px(1.), "title centred on the window");
            assert_eq!(deck.top(), bar.bounds().bottom(), "the deck's border meets the bar");
            #[cfg(target_os = "windows")]
            for id in ["minimize", "maximize", "close"] {
                let button = window.within("window-controls").find(id);
                assert!(button.visible(), "{id}");
                // A 14px icon with 20px on each side.
                assert_eq!(button.bounds().size.width, px(54.), "{id}");
            }
        });
    }

    #[gpui_kit::test]
    fn columns_reach_the_bottom_unless_the_deck_scrollbar_shows(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::runtime::Runtime::inline());
        });
        let mut deck = DeckFile::default();
        deck.add_column(FeedSource::Bookmarks);
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, AppOptions::default(), window, cx)
        });
        cx.run_until_parked();
        let gap = |cx: &mut VisualTestContext| {
            cx.run_until_parked();
            let area = cx.debug_bounds("deck-area").unwrap();
            area.bottom() - app.read_with(cx, |app, _| app.deck_scroll.bounds().bottom())
        };
        assert_eq!(gap(cx), px(0.), "one column fits: no scrollbar strip");

        app.update(cx, |app, cx| {
            for _ in 0..7 {
                app.add_column(FeedSource::Bookmarks, cx);
            }
        });
        assert_eq!(gap(cx), px(DECK_SCROLLBAR_GUTTER), "overflowing deck: strip for its bar");
    }

    #[gpui_kit::test]
    fn the_active_column_scrolls_fully_into_view(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::runtime::Runtime::inline());
        });
        let mut deck = DeckFile::default();
        for _ in 0..8 {
            deck.add_column(FeedSource::Bookmarks);
        }
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, AppOptions::default(), window, cx)
        });
        cx.run_until_parked();
        let fully_visible = |ix: usize, cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let (view, offset) = (app.deck_scroll.bounds(), app.deck_scroll.offset());
                let item = app.deck_scroll.bounds_for_item(ix).unwrap();
                item.left() + offset.x >= view.left() && item.right() + offset.x <= view.right()
            })
        };
        assert!(!fully_visible(7, cx), "the last column starts off screen");

        // What the column keys (h/l, arrows) end up calling.
        app.update(cx, |app, cx| app.set_active_column(7, cx));
        cx.run_until_parked();
        assert!(fully_visible(7, cx), "the last column is scrolled fully into view");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 7);

        app.update(cx, |app, cx| app.set_active_column(0, cx));
        cx.run_until_parked();
        assert!(fully_visible(0, cx), "and back to the first");
    }

    fn services(store: &MemoryStore, signed_in: &MockBackend) -> AccountServices {
        let backend = signed_in.clone();
        AccountServices {
            connect: Arc::new(move |_: &Credentials| -> Result<Arc<dyn Backend>, ConnectError> {
                Ok(Arc::new(backend.clone()))
            }),
            store: Arc::new(store.clone()),
            browser: None,
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
    fn the_account_button_shows_the_viewers_avatar(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: "a".into(), ct0: "c".into() }).unwrap();
        let (app, cx) = start(cx, MockBackend::new(), Some(SessionSource::Keyring), &store);
        assert!(app.read_with(cx, |app, _| app.viewer().is_some_and(|v| v.avatar.is_some())));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let face = cx.debug_bounds("account-avatar").expect("the avatar replaces the icon");
        let side = px(ACCOUNT_AVATAR);
        assert_eq!(face.size, gpui_kit::size(side, side), "round, not squeezed");
        let button = cx.debug_bounds("account-button").expect("the button is rendered");
        assert!(
            (face.center().x - button.center().x).abs() < px(1.),
            "centred: {face:?} in {button:?}"
        );

        app.update_in(cx, |app, window, cx| app.sign_out(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("account-avatar").is_none(), "signed out: no avatar");
    }

    #[gpui_kit::test]
    fn the_expanded_sidebar_names_the_viewer_beside_their_avatar(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        store.save(&Credentials { auth_token: "a".into(), ct0: "c".into() }).unwrap();
        let (app, cx) = start(cx, MockBackend::new(), Some(SessionSource::Keyring), &store);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("account-name").is_none(), "collapsed: the avatar only");
        let heading = cx.debug_bounds("rail-decks-heading").expect("headed when collapsed too");
        let deck = cx.debug_bounds("rail-deck-0").expect("deck entry drawn");
        assert!(heading.bottom() <= deck.top(), "the heading is above the decks");
        assert!(heading.size.width <= px(RAIL_WIDTH), "it fits the collapsed rail");

        app.update(cx, |app, cx| app.set_sidebar_expanded(true, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let face = cx.debug_bounds("account-avatar").expect("the avatar is drawn");
        let name = cx.debug_bounds("account-name").expect("the name is drawn");
        let handle = cx.debug_bounds("account-handle").expect("the handle is drawn");
        assert!(name.left() >= face.right(), "name right of the avatar: {name:?} {face:?}");
        assert!(handle.left() >= face.right(), "handle right of the avatar");
        assert!(handle.top() >= name.bottom(), "handle under the name: {handle:?} {name:?}");

        click(cx, "account-button");
        assert!(app.read_with(cx, |app, _| app.account_panel), "the row opens the panel");
    }

    #[gpui_kit::test]
    fn the_decks_span_the_rail_under_their_heading_and_new_deck(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        for (expanded, width) in [(false, RAIL_WIDTH), (true, RAIL_EXPANDED_WIDTH)] {
            app.update(cx, |app, cx| app.set_sidebar_expanded(expanded, cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let heading = cx.debug_bounds("rail-decks-heading").expect("heading drawn");
            let add = cx.debug_bounds("rail-deck-add").expect("new deck drawn");
            let deck = cx.debug_bounds("rail-deck-0").expect("deck drawn");
            assert!(heading.bottom() <= add.top(), "new deck under the heading ({expanded})");
            assert!(add.bottom() <= deck.top(), "then the decks ({expanded})");
            assert_eq!(deck.left(), px(0.), "from the window's edge ({expanded})");
            assert_eq!(deck.size.width, px(width), "across the whole rail ({expanded})");
        }
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
            d.choose(crate::state::add_column::ColumnKind::Bookmarks, window, cx);
            d.submit(window, cx);
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
        app.update(cx, |app, cx| app.add_deck("", DeckIcon::default(), cx));
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

    #[gpui_kit::test]
    fn the_sidebar_has_post_search_and_add_column_then_the_decks(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let top = |cx: &mut VisualTestContext, selector: &'static str| {
            cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} drawn")).top()
        };
        let order = ["rail-compose", "rail-search-people", "rail-add-column", "rail-deck-0"];
        let tops: Vec<_> = order.into_iter().map(|selector| top(cx, selector)).collect();
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "Tweet, Search people, Add column, then the decks"
        );
        assert!(top(cx, "rail-deck-add") < top(cx, "rail-deck-0"), "New deck above the list");
        assert!(cx.debug_bounds("deck-edit-0").is_none(), "no edit buttons while collapsed");

        // + asks for the new deck's name and icon first.
        click(cx, "rail-deck-add");
        let dialog = app.read_with(cx, |app, _| app.deck_settings_dialog().cloned());
        let dialog = dialog.expect("the deck dialog opened");
        assert_eq!(dialog.read_with(cx, |d, _| d.target()), DeckTarget::New);
        assert_eq!(app.read_with(cx, |app, _| app.deck().decks.len()), 1, "no deck yet");
        cx.simulate_input("News");
        click(cx, "deck-icon-newspaper");
        click(cx, "deck-settings-confirm");
        app.read_with(cx, |app, _| {
            assert!(app.deck_settings_dialog().is_none(), "closed");
            assert_eq!(app.deck().decks.len(), 2);
            assert_eq!(app.deck().active_index(), 1, "the new deck is shown");
            assert_eq!(app.deck().active().name, "News");
            assert_eq!(app.deck().active().icon, DeckIcon::Newspaper);
            assert_eq!(app.columns().count(), 0);
        });
        click(cx, "rail-deck-0");
        app.read_with(cx, |app, _| {
            assert_eq!(app.deck().active_index(), 0);
            assert_eq!(app.columns().count(), app.deck().active().columns.len());
        });
    }

    #[gpui_kit::test]
    fn the_expanded_sidebar_edits_decks_and_is_remembered(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            keys::bind(cx);
            cx.set_global(Runtime::inline());
        });
        let dir = tempfile::tempdir().unwrap();
        let (deck_path, settings_path) =
            (dir.path().join("decks.json"), dir.path().join("settings.json"));
        let options = AppOptions {
            deck_path: Some(deck_path.clone()),
            settings_path: Some(settings_path.clone()),
            ..AppOptions::default()
        };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), Settings::default(), backend, options, window, cx)
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| app.add_deck("", DeckIcon::default(), cx));

        click(cx, "rail-toggle");
        assert!(app.read_with(cx, |app, _| app.sidebar_expanded()));
        assert_eq!(Settings::load(&settings_path).unwrap().sidebar_expanded, Some(true), "saved");
        assert!(cx.debug_bounds("deck-edit-1").is_some(), "one edit button per deck");

        // The pencil opens the dialog without switching deck.
        click(cx, "deck-edit-0");
        let dialog = app.read_with(cx, |app, _| app.deck_settings_dialog().cloned());
        let dialog = dialog.expect("the dialog opened");
        assert_eq!(
            dialog.read_with(cx, |d, _| d.target()),
            DeckTarget::Existing { index: 0, removable: true }
        );
        assert_eq!(app.read_with(cx, |app, _| app.deck().active_index()), 1);

        dialog.update(cx, |d, cx| {
            d.set_name("  Work ", cx);
            d.set_icon(DeckIcon::Briefcase, cx);
        });
        cx.run_until_parked();
        let saved = DeckFile::load(&deck_path).unwrap();
        assert_eq!(
            (saved.decks[0].name.as_str(), saved.decks[0].icon),
            ("Work", DeckIcon::Briefcase)
        );
        dialog.update(cx, |d, cx| d.set_name("", cx));
        cx.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.deck().decks[0].name.clone()),
            "Work",
            "never blank"
        );

        // Remove closes the dialog; the deck that was shown stays shown.
        click(cx, "deck-remove");
        app.read_with(cx, |app, _| {
            assert!(app.deck_settings_dialog().is_none());
            assert_eq!(app.deck().decks.len(), 1);
            assert_eq!(app.deck().active().name, "Deck 2");
        });
        assert_eq!(DeckFile::load(&deck_path).unwrap().decks.len(), 1);

        click(cx, "rail-toggle");
        assert!(cx.debug_bounds("deck-edit-0").is_none(), "collapsed again");
        assert_eq!(Settings::load(&settings_path).unwrap().sidebar_expanded, Some(false));
    }

    #[gpui_kit::test]
    fn the_sidebar_opens_expanded_when_it_was_left_so(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let settings = Settings { sidebar_expanded: Some(true), ..Settings::default() };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (_, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(
                DeckFile::default(),
                settings,
                backend,
                AppOptions::default(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let rail = cx.debug_bounds("rail-deck-0").expect("deck entry drawn");
        assert!(rail.size.width > px(RAIL_WIDTH), "a full-width row");
        let heading = cx.debug_bounds("rail-decks-heading").expect("the decks are headed");
        assert!(heading.bottom() <= rail.top(), "the heading is above the decks");
        assert!(cx.debug_bounds("deck-edit-0").is_some());
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
        assert!(app.read_with(cx, |app, _| app.compose().is_none()), "closed after tweeting");
    }

    #[gpui_kit::test]
    fn profile_card_adds_a_user_column(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let post = first_post(&app, cx);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::UserColumn(Arc::clone(&post.author)), cx));
        cx.run_until_parked();
        let last = app.read_with(cx, |app, _| app.deck().active().columns.last().cloned()).unwrap();
        assert_eq!(last.source.title(), format!("@{}", post.author.handle));
        assert!(app.read_with(cx, |app, _| app.profile_panel().is_none()), "no sheet");
    }

    #[gpui_kit::test]
    fn a_mention_is_looked_up_and_opens_in_a_profile_sheet(cx: &mut TestAppContext) {
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let author = Arc::clone(&first_post(&app, cx).author);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        // Mentions can differ in case from the handle.
        let user = UserRef { id: None, handle: author.handle.to_uppercase().into() };
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Mention(user), cx));
        cx.run_until_parked();
        let panel = app.read_with(cx, |app, _| app.profile_panel().cloned()).expect("sheet open");
        assert_eq!(panel.read_with(cx, |p, _| p.profile().id.clone()), author.id);

        let unknown = UserRef { id: None, handle: "nobody_here".into() };
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Mention(unknown), cx));
        cx.run_until_parked();
        let still = app.read_with(cx, |app, _| app.profile_panel().cloned());
        assert_eq!(still, Some(panel), "an unknown account keeps the sheet");
    }

    fn open_profile(
        app: &Entity<TwittyApp>,
        cx: &mut VisualTestContext,
    ) -> (Arc<Profile>, Entity<ProfilePanel>) {
        let author = Arc::clone(&first_post(app, cx).author);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Profile(Arc::clone(&author)), cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let panel = app.read_with(cx, |app, _| app.profile_panel().cloned()).expect("sheet open");
        (author, panel)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn the_rail_searches_people_and_opens_the_picked_profile(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        click(cx, "rail-search-people");
        let dialog = app.read_with(cx, |app, _| app.user_search_dialog().cloned());
        assert!(dialog.is_some(), "the rail button opens the dialog");
        cx.simulate_input("@ada");
        cx.executor().advance_clock(crate::state::user_search::DEBOUNCE);
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        click(cx, "user-result-0");
        assert!(app.read_with(cx, |app, _| app.user_search_dialog().is_none()), "dialog closed");
        let panel = app.read_with(cx, |app, _| app.profile_panel().cloned());
        let handle = panel.map(|p| p.read_with(cx, |p, _| p.profile().handle.to_string()));
        assert_eq!(handle.as_deref(), Some("ada"));
    }

    #[gpui_kit::test]
    fn an_author_opens_in_a_profile_sheet_not_a_column(cx: &mut TestAppContext) {
        use crate::state::profile::ProfileTab;
        let (app, cx) = keyboard_app(cx);
        let columns = app.read_with(cx, |app, _| app.deck().active().columns.len());
        let (author, panel) = open_profile(&app, cx);
        assert_eq!(app.read_with(cx, |app, _| app.deck().active().columns.len()), columns);
        panel.read_with(cx, |p, _| {
            assert_eq!(p.profile().id, author.id);
            assert_eq!(p.tab(), ProfileTab::Tweets);
            let posts: Vec<_> =
                p.model(ProfileTab::Tweets).items().iter().flat_map(|i| i.posts()).collect();
            assert!(!posts.is_empty(), "the user's tweets loaded");
            assert!(!p.model(ProfileTab::Media).loaded_once(), "media waits for its tab");
        });
        assert!(cx.debug_bounds("profile-counts").is_some(), "the header shows the counts");
        // The two tabs split the sheet's width.
        let tweets = cx.debug_bounds("profile-tab-Tweets").unwrap();
        let media = cx.debug_bounds("profile-tab-Media").unwrap();
        let half = px(crate::ui::thread::PANEL_WIDTH / 2.);
        for tab in [tweets, media] {
            assert!((tab.size.width - half).abs() <= px(1.), "half the sheet: {tab:?}");
        }
        assert_eq!(tweets.right(), media.left());

        // Opening the same account again keeps the sheet (and its tab).
        click(cx, "profile-tab-Media");
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Profile(Arc::clone(&author)), cx));
        cx.run_until_parked();
        let same = app.read_with(cx, |app, _| app.profile_panel().cloned()).unwrap();
        assert_eq!(same, panel);
        assert_eq!(panel.read_with(cx, |p, _| p.tab()), ProfileTab::Media);

        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.profile_panel().is_none()), "Esc closes it");
    }

    #[gpui_kit::test]
    fn the_media_tab_is_a_gallery_that_opens_the_viewer(cx: &mut TestAppContext) {
        use crate::state::profile::ProfileTab;
        use twitty_model::UserTab;
        let (app, cx) = keyboard_app(cx);
        let (author, panel) = open_profile(&app, cx);
        click(cx, "profile-tab-Media");
        let tile = panel.read_with(cx, |p, _| {
            assert_eq!(p.tab(), ProfileTab::Media);
            assert!(matches!(
                p.model(ProfileTab::Media).source(),
                twitty_model::FeedSource::User { id, tab: UserTab::Media, .. } if *id == author.id
            ));
            assert!(p.tiles().iter().all(|t| t.post.author.id == author.id));
            p.tiles().get(1).cloned().expect("the user has media")
        });

        click(cx, "gallery-tile-1");
        assert_eq!(
            app.read_with(cx, |app, _| app.media_viewer().map(|(p, ix)| (p.id.clone(), ix))),
            Some((tile.post.id.clone(), tile.index))
        );
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_none()), "viewer first");
        assert!(app.read_with(cx, |app, _| app.profile_panel().is_some()), "sheet stays");
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.profile_panel().is_none()));
    }

    #[gpui_kit::test]
    fn a_post_in_the_profile_opens_its_thread_over_it(cx: &mut TestAppContext) {
        use crate::state::profile::ProfileTab;
        let (app, cx) = keyboard_app(cx);
        let (_, panel) = open_profile(&app, cx);
        click(cx, "profile-tab-Media");
        let id = panel.read_with(cx, |p, _| {
            let items = p.model(ProfileTab::Tweets).items();
            items.iter().flat_map(|i| i.posts()).next().map(|p| p.id.clone()).unwrap()
        });
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(id.clone()), cx));
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.profile_panel().is_none()), "the thread is on top");
        assert_eq!(panel_root(&app, cx), Some(id.to_string()));
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 2);

        // The tweet already on top doesn't stack a copy.
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(id.clone()), cx));
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 2);

        // Esc goes back to the same profile, on the tab it was on.
        cx.simulate_keystrokes("escape");
        assert_eq!(app.read_with(cx, |app, _| app.profile_panel().cloned()), Some(panel.clone()));
        assert_eq!(panel.read_with(cx, |p, _| p.tab()), ProfileTab::Media);
        cx.simulate_keystrokes("escape");
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 0);
    }

    #[gpui_kit::test]
    fn a_sheet_under_another_keeps_its_scroll_position(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let (_, panel) = open_profile(&app, cx);
        let viewport = cx.update(|window, _| window.viewport_size());
        for _ in 0..5 {
            cx.simulate_event(ScrollWheelEvent {
                position: point(viewport.width - px(300.), px(400.)),
                delta: ScrollDelta::Lines(point(0.0, -10.0)),
                ..Default::default()
            });
            cx.run_until_parked();
        }
        let scrolled = panel.read_with(cx, |p, _| p.scroll_top());
        assert!(scrolled.0 > 0, "the profile scrolled: {scrolled:?}");

        open_panel(&app, "1", cx);
        cx.simulate_keystrokes("escape");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.profile_panel().cloned()), Some(panel.clone()));
        assert_eq!(panel.read_with(cx, |p, _| p.scroll_top()), scrolled);
    }

    /// Stacks threads on `roots`, the last on top, with animations off so the
    /// sheets are drawn where they end up.
    fn stack_threads(app: &Entity<TwittyApp>, roots: &[&str], cx: &mut VisualTestContext) {
        app.update(cx, |app, cx| {
            app.add_deck("", DeckIcon::default(), cx);
            app.set_animations(false, cx);
        });
        for root in roots {
            open_panel(app, root, cx);
        }
    }

    #[gpui_kit::test]
    fn lower_sheets_show_their_edge_and_a_press_there_closes_the_top(cx: &mut TestAppContext) {
        use crate::state::sheets::{MAX_PEEKS, PEEK};
        let (app, cx) = keyboard_app(cx);
        stack_threads(&app, &["1", "2", "3", "4", "5", "6", "7", "8"], cx);
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 8);
        const NAMES: [&str; 7] =
            ["sheet-0", "sheet-1", "sheet-2", "sheet-3", "sheet-4", "sheet-5", "sheet-6"];
        assert!(MAX_PEEKS < NAMES.len());
        let sheet = |cx: &mut VisualTestContext, depth: usize| cx.debug_bounds(NAMES[depth]);
        let top = sheet(cx, 0).expect("top sheet drawn");
        assert_eq!(top.size.width, px(crate::ui::thread::PANEL_WIDTH));
        for depth in 1..=MAX_PEEKS {
            let lower = sheet(cx, depth).expect("lower sheet drawn");
            assert_eq!(top.right() - lower.right(), px(PEEK * depth as f32), "depth {depth}");
            assert_eq!(lower.size.width, top.size.width);
        }
        assert!(sheet(cx, MAX_PEEKS + 1).is_none(), "deeper sheets are hidden behind");
        assert_eq!(cx.debug_bounds("sheet-edge-0"), None, "the top sheet is live");

        // The visible strip of the sheet two levels down.
        let two = sheet(cx, 2).unwrap();
        let strip = point(two.left() + px(PEEK / 2.), px(300.));
        cx.simulate_click(strip, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 7, "only the top one closed");
        assert_eq!(panel_root(&app, cx).as_deref(), Some("7"), "nothing inside it reacted");
    }

    #[gpui_kit::test]
    fn close_and_a_click_beside_shut_only_the_top_sheet(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        stack_threads(&app, &["1", "2"], cx);
        let top = app.read_with(cx, |app, _| app.thread_panel().cloned()).unwrap();
        top.update(cx, |_, cx| cx.emit(CloseThread));
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx).as_deref(), Some("1"), "× shows the one under it");

        open_panel(&app, "2", cx);
        open_panel(&app, "3", cx);
        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx).as_deref(), Some("2"), "a click beside closes the top");
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 2);
    }

    #[gpui_kit::test]
    fn a_click_beside_a_thread_over_a_profile_goes_back_to_the_profile(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let (_, panel) = open_profile(&app, cx);
        let id = panel.read_with(cx, |p, _| {
            let items = p.model(crate::state::profile::ProfileTab::Tweets).items();
            items.iter().flat_map(|i| i.posts()).next().map(|p| p.id.clone()).unwrap()
        });
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(id), cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 2);

        // Over the deck's tweets, not an empty deck.
        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.profile_panel().cloned()), Some(panel.clone()));
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 1);
        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 0);
    }

    #[gpui_kit::test]
    fn scrolling_the_top_sheet_does_not_redraw_the_ones_under_it(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        stack_threads(&app, &["1"], cx);
        let lower = app.read_with(cx, |app, _| app.thread_panel().cloned()).unwrap();
        open_panel(&app, "2", cx);
        let top = app.read_with(cx, |app, _| app.thread_panel().cloned()).unwrap();
        let renders = |cx: &mut VisualTestContext| {
            (lower.read_with(cx, |p, _| p.renders()), top.read_with(cx, |p, _| p.renders()))
        };
        let (lower_before, top_before) = renders(cx);
        // What a scroll does: the top panel notifies, its ancestors redraw.
        for _ in 0..3 {
            top.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
        let (lower_after, top_after) = renders(cx);
        assert!(top_after > top_before, "the top sheet redrew");
        assert_eq!(lower_after, lower_before, "the sheet under it reused its frame");

        // A change in the lower sheet itself still shows.
        lower.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let (lower_now, _) = renders(cx);
        assert!(lower_now > lower_before);

        // So does a new media height, which only a global carries.
        app.update(cx, |app, cx| app.set_media_max_height(Some(MaxHeight::new(200.)), false, cx));
        cx.run_until_parked();
        assert!(renders(cx).0 > lower_now, "the lower sheet redrew with the new height");
    }

    #[gpui_kit::test]
    fn closed_sheets_slide_out_from_their_place(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        app.update(cx, |app, cx| app.add_deck("", DeckIcon::default(), cx));
        open_panel(&app, "1", cx);
        open_panel(&app, "2", cx);
        cx.simulate_keystrokes("escape");
        let leaving = |app: &Entity<TwittyApp>, cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let leaving = app.leaving_sheet.as_ref().map(|l| l.what.as_slice()).unwrap_or(&[]);
                leaving.iter().map(|(_, depth)| *depth).collect::<Vec<_>>()
            })
        };
        assert_eq!(leaving(&app, cx), vec![0], "the top one leaves");
        assert_eq!(panel_root(&app, cx).as_deref(), Some("1"));
        // Slower than a dialog's exit.
        let motion = cx.update(|_, cx| cx.theme().motion.clone());
        cx.executor().advance_clock(motion.duration_fast + std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(leaving(&app, cx), vec![0], "still sliding out");
        cx.executor().advance_clock(motion.duration_slow - motion.duration_fast);
        cx.run_until_parked();
        assert!(!app.read_with(cx, |app, _| app.is_leaving()), "gone when its animation ends");
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));

        // A click beside it closes the top one too, and the last one with
        // its backdrop.
        cx.simulate_click(OUTSIDE, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(leaving(&app, cx), vec![0]);
        assert_eq!(app.read_with(cx, |app, _| app.sheet_count()), 0);
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));

        // Two quick Esc: the first sheet keeps sliding out, over the second.
        open_panel(&app, "1", cx);
        open_panel(&app, "2", cx);
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("escape");
        let roots = app.read_with(cx, |app, cx| {
            let leaving = app.leaving_sheet.as_ref().map(|l| l.what.as_slice()).unwrap_or(&[]);
            leaving
                .iter()
                .map(|(sheet, _)| match sheet {
                    Sheet::Thread(panel) => panel.read(cx).root().to_string(),
                    Sheet::Profile(_) => "profile".into(),
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(roots, ["1", "2"], "the one closed last is drawn first");
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));
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
        fn posts<'a>(
            &'a self,
            ids: &'a [twitty_model::PostId],
        ) -> twitty_model::BoxFuture<'a, Result<Vec<twitty_model::Post>, BackendError>> {
            self.0.posts(ids)
        }
        fn profile<'a>(
            &'a self,
            handle: &'a str,
        ) -> twitty_model::BoxFuture<'a, Result<Profile, BackendError>> {
            self.0.profile(handle)
        }
        fn search_users<'a>(
            &'a self,
            query: &'a str,
        ) -> twitty_model::BoxFuture<'a, Result<Vec<Profile>, BackendError>> {
            self.0.search_users(query)
        }
        fn viewer_lists(
            &self,
        ) -> twitty_model::BoxFuture<'_, Result<Vec<twitty_model::ListSummary>, BackendError>>
        {
            self.0.viewer_lists()
        }
        fn set_following<'a>(
            &'a self,
            user: &'a twitty_model::UserId,
            action: twitty_model::FollowAction,
        ) -> twitty_model::BoxFuture<'a, Result<twitty_model::FollowStatus, BackendError>> {
            self.0.set_following(user, action)
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
        // Every column polls except the default deck's "For you".
        let sources = app.read_with(cx, |app, cx| {
            app.columns().map(|c| c.read(cx).model().source().clone()).collect::<Vec<_>>()
        });
        for (source, every) in sources.iter().zip(&got) {
            let for_you = *source == FeedSource::Home { mode: twitty_model::HomeMode::ForYou };
            assert_eq!(every.is_none(), for_you, "{source:?}");
        }

        app.update(cx, |app, cx| {
            app.settings.polling = Some(false);
            app.update_polling(cx);
        });
        assert!(intervals(cx).iter().all(Option::is_none), "polling can be turned off");
    }

    #[gpui_kit::test]
    fn counters_of_visible_tweets_are_refreshed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let mut deck = DeckFile::default();
        deck.add_column(FeedSource::Bookmarks);
        let options = AppOptions { poll: true, ..AppOptions::default() };
        let mock = MockBackend::new();
        let backend: Arc<dyn Backend> = Arc::new(mock.clone());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(deck, Settings::default(), backend, options, window, cx)
        });
        cx.run_until_parked();
        let first = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                let column = app.columns().next().unwrap().read(cx);
                column.visible_items()[0].posts()[0].clone()
            })
        };
        let post = first(cx);
        let metrics = twitty_model::Metrics { likes: 4321, views: Some(98_765), ..post.metrics };
        mock.set_metrics(&post.id, metrics);
        assert!(mock.lookups().is_empty(), "nothing before the first period");

        cx.executor().advance_clock(stats::REFRESH_EVERY);
        cx.run_until_parked();
        let lookups = mock.lookups();
        assert_eq!(lookups.len(), 1, "one request for the whole deck");
        assert!(lookups[0].contains(&post.id));
        assert!(lookups[0].len() <= stats::MAX_IDS);
        let shown = app.read_with(cx, |app, cx| app.hub.read(cx).effective(&post).metrics);
        assert_eq!(shown, metrics);

        app.update(cx, |app, _| app.settings.polling = Some(false));
        cx.executor().advance_clock(stats::REFRESH_EVERY);
        cx.run_until_parked();
        assert_eq!(mock.lookups().len(), 1, "turning polling off stops the refreshes");
    }

    fn keyboard_app(cx: &mut TestAppContext) -> (Entity<TwittyApp>, &mut VisualTestContext) {
        keyboard_app_on(MockBackend::new(), cx)
    }

    fn keyboard_app_on(
        backend: MockBackend,
        cx: &mut TestAppContext,
    ) -> (Entity<TwittyApp>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            keys::bind(cx);
            cx.set_global(Runtime::inline());
        });
        let backend: Arc<dyn Backend> = Arc::new(backend);
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
        assert_eq!(selected(&app, cx), None, "nothing selected at launch");
        cx.simulate_keystrokes("down");
        assert_eq!(selected(&app, cx), Some(0), "the first press selects the first tweet");
        cx.simulate_keystrokes("up");
        assert_eq!(selected(&app, cx), Some(0));
        cx.simulate_keystrokes("j j j k");
        assert_eq!(selected(&app, cx), Some(2));
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 1);
        assert_eq!(selected(&app, cx), Some(0), "the next column starts on its first tweet");
        cx.simulate_keystrokes("j");
        assert_eq!(selected(&app, cx), Some(1));
        cx.simulate_keystrokes("left");
        assert_eq!(selected(&app, cx), Some(2), "a column keeps its selection");
        cx.simulate_keystrokes("left left left");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0, "clamped");
    }

    #[gpui_kit::test]
    fn escape_resets_keyboard_navigation(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("down down right down escape");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0);
        assert_eq!(selected(&app, cx), None);
        let second = app.read_with(cx, |app, _| app.columns[1].1.clone());
        assert_eq!(second.read_with(cx, |c, _| c.model().selected()), None);
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0, "like at launch");
        assert_eq!(selected(&app, cx), Some(0));
    }

    #[gpui_kit::test]
    fn a_first_right_press_selects_the_first_post_in_place(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0, "no column change");
        assert_eq!(selected(&app, cx), Some(0));
        cx.simulate_keystrokes("right");
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 1);
        assert_eq!(selected(&app, cx), Some(0));
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

    fn toast_texts(app: &Entity<TwittyApp>, cx: &mut VisualTestContext) -> Vec<String> {
        app.read_with(cx, |app, _| app.toasts().items().iter().map(|t| t.text.clone()).collect())
    }

    #[gpui_kit::test]
    fn a_bookmark_shows_a_toast_that_hides_by_itself(cx: &mut TestAppContext) {
        use crate::state::toasts::{EXIT, SUCCESS_TTL};
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("j b");
        cx.run_until_parked();
        assert_eq!(toast_texts(&app, cx), ["Added to Bookmarks"]);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let toasts = cx.debug_bounds("toasts").expect("the toast is drawn");
        let window = cx.debug_bounds("app-content").expect("content is drawn");
        assert_eq!(window.right() - toasts.right(), px(16.), "16px from the right edge");
        assert_eq!(window.bottom() - toasts.bottom(), px(16.), "16px from the bottom edge");

        cx.executor().advance_clock(SUCCESS_TTL);
        cx.run_until_parked();
        assert_eq!(toast_texts(&app, cx).len(), 1, "fading out");
        cx.executor().advance_clock(EXIT);
        cx.run_until_parked();
        assert!(toast_texts(&app, cx).is_empty());
    }

    #[gpui_kit::test]
    fn a_failed_action_shows_why(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        let (app, cx) = keyboard_app_on(backend.clone(), cx);
        // After the columns loaded: the like is the next call.
        backend.fail_next(BackendError::Upstream("Twitter error 226: automated".into()));
        cx.simulate_keystrokes("j l");
        cx.run_until_parked();
        assert_eq!(toast_texts(&app, cx), ["Couldn't like: Twitter error 226: automated"]);
        let kind = app.read_with(cx, |app, _| app.toasts().items()[0].kind);
        assert_eq!(kind, crate::state::toasts::ToastKind::Error);
    }

    /// Lets a just-opened dialog take the keyboard.
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn t_asks_before_reposting_and_holds_the_keyboard(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        let (app, cx) = keyboard_app_on(backend.clone(), cx);
        cx.simulate_keystrokes("j t");
        draw(cx);
        let dialog = app.read_with(cx, |app, _| app.repost_dialog().cloned()).expect("asks first");
        let post = dialog.read_with(cx, |d, _| Arc::clone(d.post()));
        assert_eq!(dialog.read_with(cx, |d, _| d.choice()), RepostChoice::Repost);
        assert!(backend.actions().is_empty(), "nothing sent before confirming");

        // Deck shortcuts don't act behind the dialog.
        cx.simulate_keystrokes("j l t");
        assert_eq!(selected(&app, cx), Some(0));
        assert!(backend.actions().is_empty());

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.repost_dialog().is_none()), "closed");
        assert_eq!(backend.actions(), [twitty_model::PostAction::Repost(post.id.clone())]);
        assert!(app.read_with(cx, |app, cx| app.hub.read(cx).effective(&post).viewer.reposted));

        // Asked again, it offers to undo; Esc changes nothing.
        draw(cx);
        cx.simulate_keystrokes("t");
        draw(cx);
        let dialog = app.read_with(cx, |app, _| app.repost_dialog().cloned()).expect("asks again");
        assert_eq!(dialog.read_with(cx, |d, _| d.choice()), RepostChoice::Undo);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.repost_dialog().is_none()));
        assert_eq!(backend.actions().len(), 1);

        draw(cx);
        cx.simulate_keystrokes("t");
        draw(cx);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(backend.actions()[1], twitty_model::PostAction::Unrepost(post.id.clone()));
    }

    #[gpui_kit::test]
    fn the_repost_button_quotes_from_its_dialog(cx: &mut TestAppContext) {
        let backend = MockBackend::new();
        let (app, cx) = keyboard_app_on(backend.clone(), cx);
        let post = first_post(&app, cx);
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Repost(Arc::clone(&post)), cx));
        draw(cx);
        assert!(app.read_with(cx, |app, _| app.repost_dialog().is_some()));
        cx.simulate_keystrokes("q");
        draw(cx);
        assert!(app.read_with(cx, |app, _| app.repost_dialog().is_none()));
        let compose = app.read_with(cx, |app, _| app.compose().cloned()).expect("compose open");
        assert_eq!(
            compose.read_with(cx, |c, _| c.target().clone()),
            ComposeTarget::Quote(Arc::clone(&post))
        );
        // The text field has the keyboard, so `q` and the rest are typed.
        cx.simulate_keystrokes("q j");
        assert_eq!(compose.read_with(cx, |c, cx| c.text_value(cx)), "qj");
        compose.update(cx, |c, cx| c.submit(cx));
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.compose().is_none()), "closed after tweeting");
        assert!(backend.actions().is_empty(), "a quote is a tweet, not a retweet");
        assert_eq!(toast_texts(&app, cx), ["Quote sent"]);
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
        .expect("the mock has a multi-image tweet");
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

    #[gpui_kit::test]
    fn a_gif_plays_in_the_media_viewer_until_it_is_left(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let video = cx
            .update(|_, cx| crate::video::FakeVideo::install(vec![std::time::Duration::ZERO], cx));
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let mut post = column
            .read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| !p.media.is_empty())
                    .cloned()
            })
            .expect("the mock has a tweet with media");
        let poster = post.media[0].preview().clone();
        let stream = poster.url.join("/clip.mp4").unwrap();
        post.media = vec![
            twitty_model::Media::Gif {
                poster: poster.clone(),
                stream: Some(stream.clone()),
                alt: None,
            },
            twitty_model::Media::Image { image: poster, alt: None },
        ];
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let to = Navigate::Media { post: Arc::new(post), index: 0 };
        hub.update(cx, |hub, cx| hub.navigate(to, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let player = app.read_with(cx, |app, _| app.media_player().cloned()).expect("playing");
        assert!(player.read_with(cx, |p, _| p.current().is_some()), "a frame is drawn");
        assert_eq!(video.opened(), vec![stream.to_string()]);

        let at = cx.debug_bounds("media-player").expect("the GIF is drawn").center();
        cx.simulate_click(at, Modifiers::default());
        assert_eq!(player.read_with(cx, |p, _| p.state()), State::Paused, "a click pauses");
        cx.simulate_click(at, Modifiers::default());
        assert_eq!(player.read_with(cx, |p, _| p.state()), State::Playing, "and resumes");
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_some()), "not the backdrop");

        cx.simulate_keystrokes("right");
        assert!(app.read_with(cx, |app, _| app.media_player().is_none()), "an image: no player");
        cx.simulate_keystrokes("left");
        assert!(app.read_with(cx, |app, _| app.media_player().is_some()), "back on the GIF");
        assert_eq!(video.opened().len(), 2, "it starts over");
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.media_player().is_none()), "closing stops it");
    }

    #[gpui_kit::test]
    fn a_video_plays_with_sound_and_controls_in_the_media_viewer(cx: &mut TestAppContext) {
        use std::time::Duration;
        let (app, cx) = keyboard_app(cx);
        let frames = vec![Duration::ZERO, Duration::from_secs(60)];
        let video = cx.update(|_, cx| crate::video::FakeVideo::install(frames, cx));
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let mut post = column
            .read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| !p.media.is_empty())
                    .cloned()
            })
            .expect("the mock has a tweet with media");
        let poster = post.media[0].preview().clone();
        let stream = Some(poster.url.join("/clip.mp4").unwrap());
        let duration = Some(Duration::from_secs(42));
        let variants = stream
            .into_iter()
            .map(|url| twitty_model::VideoVariant { url, bitrate: None, size: None })
            .collect();
        post.media = vec![twitty_model::Media::Video { poster, variants, duration, alt: None }];
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let to = Navigate::Media { post: Arc::new(post), index: 0 };
        hub.update(cx, |hub, cx| hub.navigate(to, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        assert_eq!(video.sounds(), vec![crate::video::Sound::On]);
        let player = app.read_with(cx, |app, _| app.media_player().cloned()).expect("playing");
        assert_eq!(player.read_with(cx, |p, _| p.duration()), duration, "the model's, until told");
        assert!(cx.debug_bounds("video-controls").is_some(), "the control bar");
        assert!(cx.debug_bounds("media-play").is_none(), "no need for x.com");

        let play = cx.debug_bounds("video-play").expect("the play button").center();
        cx.simulate_click(play, Modifiers::default());
        assert_eq!(player.read_with(cx, |p, _| p.state()), State::Paused, "toggled once");
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_some()), "the viewer stays");
        assert_eq!(video.control(0).unwrap().paused(), Some(true));
        cx.simulate_keystrokes("escape");
        drop(player);
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(video.listening(), 0, "closing stops the playback");
    }

    /// Opens the media viewer on a 42 s video and returns its player.
    fn viewer_video(
        app: &Entity<TwittyApp>,
        cx: &mut VisualTestContext,
    ) -> (Entity<VideoPlayer>, Arc<crate::video::FakeVideo>) {
        use std::time::Duration;
        let frames = vec![Duration::ZERO, Duration::from_secs(60)];
        let video = cx.update(|_, cx| crate::video::FakeVideo::install(frames, cx));
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let mut post = column
            .read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| !p.media.is_empty())
                    .cloned()
            })
            .expect("the mock has a tweet with media");
        let poster = post.media[0].preview().clone();
        let stream = Some(poster.url.join("/clip.mp4").unwrap());
        let duration = Some(Duration::from_secs(42));
        let variants = stream
            .into_iter()
            .map(|url| twitty_model::VideoVariant { url, bitrate: None, size: None })
            .collect();
        post.media = vec![twitty_model::Media::Video { poster, variants, duration, alt: None }];
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let to = Navigate::Media { post: Arc::new(post), index: 0 };
        hub.update(cx, |hub, cx| hub.navigate(to, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let player = app.read_with(cx, |app, _| app.media_player().cloned()).expect("playing");
        (player, video)
    }

    #[gpui_kit::test]
    fn a_seek_drag_released_outside_the_bar_only_seeks(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let (player, video) = viewer_video(&app, cx);
        let slider = cx.debug_bounds("video-progress").expect("the progress slider");
        let picture = cx.debug_bounds("media-player").expect("the video");
        use gpui_kit::MouseButton;
        let drag = |from, to, cx: &mut VisualTestContext| {
            cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
        };
        // Up over the picture, which pauses on a click.
        drag(slider.center(), picture.center(), cx);
        assert_eq!(player.read_with(cx, |p, _| p.state()), State::Playing, "no pause");
        assert_eq!(video.control(0).unwrap().seeks().len(), 1, "one seek, on release");
        // Up below the video, over the backdrop, which closes on a click.
        let quarter = point(slider.left() + slider.size.width / 4., slider.center().y);
        drag(quarter, point(picture.left() + px(30.), picture.bottom() + px(20.)), cx);
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_some()), "still open");
        assert_eq!(video.control(0).unwrap().seeks().len(), 2);
    }

    #[gpui_kit::test]
    fn a_gif_that_fails_to_play_falls_back_to_x_com(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.update(|_, cx| crate::video::FakeVideo::install_failing(cx));
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let mut post = column
            .read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| !p.media.is_empty())
                    .cloned()
            })
            .expect("the mock has a tweet with media");
        let poster = post.media[0].preview().clone();
        let stream = Some(poster.url.join("/missing.mp4").unwrap());
        post.media = vec![twitty_model::Media::Gif { poster, stream, alt: None }];
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let to = Navigate::Media { post: Arc::new(post), index: 0 };
        hub.update(cx, |hub, cx| hub.navigate(to, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, cx| app.media_player().unwrap().read(cx).failed()));
        assert!(cx.debug_bounds("media-play").is_some(), "the fallback button is back");
    }

    #[gpui_kit::test]
    fn clicking_beside_the_picture_closes_the_media_viewer(cx: &mut TestAppContext) {
        use image::{ImageFormat, Rgba, RgbaImage};
        // A very wide bitmap: shrunk to the box's width, it is far shorter
        // than its own 400 px.
        let mut png = std::io::Cursor::new(Vec::new());
        RgbaImage::from_pixel(4000, 400, Rgba([255, 0, 0, 255]))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let (app, cx) = keyboard_app(cx);
        cx.update(|_, cx| {
            cx.set_http_client(gpui_kit::http_client::FakeHttpClient::create(move |_| {
                let body = png.clone();
                async move {
                    Ok(gpui_kit::http_client::Response::builder()
                        .status(200)
                        .body(gpui_kit::http_client::AsyncBody::from(body))?)
                }
            }));
        });
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let mut post = column
            .read_with(cx, |c, _| {
                c.model()
                    .items()
                    .iter()
                    .flat_map(|i| i.posts())
                    .find(|p| !p.media.is_empty())
                    .cloned()
            })
            .expect("the mock has a tweet with media");
        // Any URL: the client serves the wide bitmap.
        let image = post.media[0].preview().clone();
        post.media = vec![twitty_model::Media::Image { image, alt: None }];
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        let post = Arc::new(post);
        let open = |cx: &mut VisualTestContext| {
            let to = Navigate::Media { post: Arc::clone(&post), index: 0 };
            hub.update(cx, |hub, cx| hub.navigate(to, cx));
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            cx.debug_bounds("media-image").expect("image drawn")
        };

        let image = open(cx);
        let aspect = image.size.width / image.size.height;
        assert!((aspect - 10.0).abs() < 0.1, "the element is the picture: {image:?}");
        cx.simulate_click(image.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_some()), "the picture keeps it");

        // Above the picture, where GPUI used to letterbox it inside the element.
        let above = point(image.center().x, image.top() - px(10.));
        assert!(image.size.height < px(380.), "room above: {image:?}");
        cx.simulate_click(above, Modifiers::default());
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_none()), "beside it closes");
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
        app.update(cx, |app, cx| app.add_deck("", DeckIcon::default(), cx));
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
        let post = Arc::new(post.expect("the mock has a tweet with media"));
        app.update(cx, |app, cx| app.add_deck("", DeckIcon::default(), cx));
        open_panel(&app, "1", cx);
        // e.g. an image opened from a tweet in the panel.
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

        // Over the first tweet's text: it doesn't open its thread either.
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
        let post = Arc::new(post.expect("the mock has a multi-image tweet"));
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

        // The strips run along the whole screen edges, corners included.
        cx.simulate_click(point(px(2.), title_bar::HEIGHT + px(2.)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(index(cx), Some(0), "the top-left corner steps back");
        cx.simulate_click(
            point(viewport.width - px(2.), viewport.height - px(2.)),
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(index(cx), Some(1), "the bottom-right corner steps on");

        // The close button stays on top of the › strip.
        cx.simulate_click(
            point(
                viewport.width - px(viewer::PADDING + 16.),
                title_bar::HEIGHT + px(viewer::PADDING + viewer::HEADER / 2.),
            ),
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(index(cx), None, "the close button closes");

        // The padding above everything (below the title bar) is backdrop.
        let post = column.read_with(cx, |c, _| {
            c.model().items().iter().flat_map(|i| i.posts()).find(|p| p.media.len() >= 2).cloned()
        });
        let post = Arc::new(post.expect("the mock has a multi-image tweet"));
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Media { post, index: 0 }, cx));
        cx.run_until_parked();
        assert_eq!(index(cx), Some(0));
        cx.simulate_click(
            point(viewport.width / 2., title_bar::HEIGHT + px(viewer::PADDING / 2.)),
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(index(cx), None);
    }

    /// Opens one kind of dialog (some need the column's id).
    type OpenDialog = fn(&mut TwittyApp, ColumnId, &mut Window, &mut Context<TwittyApp>);

    #[gpui_kit::test]
    fn a_press_beside_a_dialog_closes_it_but_not_one_inside(cx: &mut TestAppContext) {
        // With account services, so the sign-in dialog opens too.
        let store = MemoryStore::default();
        let (app, cx) = start(cx, MockBackend::new(), None, &store);
        let column = app.read_with(cx, |app, _| app.columns[0].0);
        let openers: [(&str, OpenDialog); 8] = [
            ("compose", |app, _, window, cx| app.open_compose(ComposeTarget::New, window, cx)),
            ("repost", |app, _, window, cx| {
                let column = app.columns[0].1.read(cx);
                let post = column.model().items().iter().flat_map(|i| i.posts()).next().cloned();
                app.open_repost(Arc::new(post.expect("the column has tweets")), window, cx);
            }),
            ("search people", |app, _, window, cx| app.open_user_search(window, cx)),
            ("add column", |app, _, window, cx| app.open_add_column(window, cx)),
            ("column settings", |app, id, window, cx| app.open_column_settings(id, window, cx)),
            ("login", |app, _, window, cx| app.open_login(None, window, cx)),
            ("settings", |app, _, _, cx| app.open_settings(cx)),
            ("account", |app, _, _, cx| {
                app.account_panel = true;
                cx.notify();
            }),
        ];
        let has_dialog = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.has_dialog());
        for (name, open) in openers {
            cx.update(|window, cx| app.update(cx, |app, cx| open(app, column, window, cx)));
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            assert!(has_dialog(cx), "{name} opened");

            // The card's own padding, clear of its controls.
            let card = cx.debug_bounds("overlay-card").expect("the card is drawn");
            cx.simulate_click(card.origin + point(px(4.), px(4.)), Modifiers::default());
            cx.run_until_parked();
            assert!(has_dialog(cx), "a press inside {name} keeps it open");

            // The backdrop's corner, below the title bar.
            cx.simulate_click(point(px(10.), title_bar::HEIGHT + px(10.)), Modifiers::default());
            cx.run_until_parked();
            assert!(!has_dialog(cx), "a press beside {name} closes it");
            assert_eq!(panel_root(&app, cx), None, "the press reaches nothing under {name}");
            finish_exits(cx);
        }
    }

    /// Lets every exit animation finish.
    fn finish_exits(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_closing_thread_panel_animates_out_without_taking_clicks(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        // A spot in the deck where a click opens a thread (not, say, a profile).
        let in_deck = (60..1000)
            .step_by(20)
            .map(|y| point(px(250.), px(y as f32)))
            .find(|&at| {
                cx.simulate_click(at, Modifiers::default());
                cx.run_until_parked();
                let opened = panel_root(&app, cx).is_some();
                app.update(cx, |app, cx| app.dismiss(cx));
                finish_exits(cx);
                opened
            })
            .expect("the deck shows a tweet to click");
        // A spot inside the open panel where a click switches to another tweet.
        let root = first_post(&app, cx).id.to_string();
        let viewport = cx.update(|window, _| window.viewport_size());
        let x = viewport.width - px(300.);
        let spot = (60..1000)
            .step_by(20)
            .map(|y| point(x, px(y as f32)))
            .find(|&at| {
                open_panel(&app, &root, cx);
                cx.simulate_click(at, Modifiers::default());
                cx.run_until_parked();
                let switched = panel_root(&app, cx).is_some_and(|now| now != root);
                app.update(cx, |app, cx| {
                    app.dismiss(cx);
                    app.dismiss(cx);
                });
                finish_exits(cx);
                switched
            })
            .expect("the panel shows a tweet to click");

        open_panel(&app, &root, cx);
        cx.simulate_keystrokes("escape");
        assert_eq!(panel_root(&app, cx), None, "closed at once");
        assert!(app.read_with(cx, |app, _| app.is_leaving()), "still drawn while it slides out");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        // The same spot on the leaving panel does nothing...
        cx.simulate_click(spot, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(panel_root(&app, cx), None, "the leaving panel's tweets don't react");
        // ...but the deck beside it does: a tweet opens its thread.
        cx.simulate_click(in_deck, Modifiers::default());
        cx.run_until_parked();
        assert!(panel_root(&app, cx).is_some(), "the leaving panel doesn't block the deck");
        assert!(!app.read_with(cx, |app, _| app.is_leaving()), "reopening drops the old one");

        cx.simulate_keystrokes("escape");
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()), "gone once the animation is over");
    }

    #[gpui_kit::test]
    fn a_closing_image_viewer_is_inert(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        let column = app.read_with(cx, |app, _| app.active_view()).unwrap();
        let post = column.read_with(cx, |c, _| {
            c.model().items().iter().flat_map(|i| i.posts()).find(|p| p.media.len() >= 2).cloned()
        });
        let post = Arc::new(post.expect("the mock has a multi-image tweet"));
        let hub = app.read_with(cx, |app, _| app.hub.clone());
        hub.update(cx, |hub, cx| hub.navigate(Navigate::Media { post, index: 0 }, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.is_leaving()));

        // The fading › button does nothing (it would step the columns now).
        let viewport = cx.update(|window, _| window.viewport_size());
        let next =
            point(viewport.width - px(viewer::PADDING + viewer::NAV / 2.), viewport.height / 2.);
        cx.simulate_click(next, Modifiers::default());
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.media_viewer().is_none()));
        assert_eq!(app.read_with(cx, |app, _| app.active_column()), 0);
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));
    }

    #[gpui_kit::test]
    fn closing_dialogs_animate_out_unless_animations_are_off(cx: &mut TestAppContext) {
        let (app, cx) = keyboard_app(cx);
        cx.simulate_keystrokes("n");
        cx.simulate_keystrokes("escape");
        assert!(app.read_with(cx, |app, _| app.compose().is_none()));
        assert!(app.read_with(cx, |app, _| app.is_leaving()));
        finish_exits(cx);
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));

        app.update(cx, |app, cx| app.set_animations(false, cx));
        assert!(cx.update(|_, cx| cx.reduce_motion()));
        app.update(cx, |app, cx| app.open_settings(cx));
        cx.simulate_keystrokes("escape");
        assert!(!app.read_with(cx, |app, _| app.is_leaving()), "nothing lingers");
        open_panel(&app, "1", cx);
        cx.simulate_keystrokes("escape");
        assert!(!app.read_with(cx, |app, _| app.is_leaving()));
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
        app.update(cx, |app, cx| app.set_animations(false, cx));
        assert_eq!(cx.update(|_, cx| crate::ui::gif::autoplay(cx)), Autoplay::Off, "default");
        app.update(cx, |app, cx| app.set_gif_autoplay(true, cx));
        assert_eq!(cx.update(|_, cx| crate::ui::gif::autoplay(cx)), Autoplay::On, "applies live");
        cx.run_until_parked();
        assert_eq!(app.read_with(cx, |app, _| app.theme_name()), "Light");
        let polling = app.read_with(cx, |app, cx| {
            app.columns().map(|c| c.read(cx).poll_interval()).collect::<Vec<_>>()
        });
        assert!(polling.iter().all(Option::is_none));
        let saved = Settings::load(&path).unwrap();
        assert_eq!(
            (saved.theme.as_deref(), saved.polling, saved.animations, saved.gif_autoplay),
            (Some("Light"), Some(false), Some(false), Some(true))
        );
        assert!(cx.update(|_, cx| cx.reduce_motion()), "animations off is GPUI's reduce motion");
        cx.simulate_keystrokes("escape");
    }

    #[gpui_kit::test]
    fn the_media_height_cap_applies_live_and_is_saved(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(theme::NAMES[0], cx);
            cx.set_global(Runtime::inline());
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let options = AppOptions { settings_path: Some(path.clone()), ..AppOptions::default() };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), Settings::default(), backend, options, window, cx)
        });
        cx.run_until_parked();
        // The heights of the media grids drawn in the first rows.
        let grids = |cx: &mut gpui_kit::VisualTestContext| {
            const GRIDS: [&str; 12] = [
                "media-0", "media-1", "media-2", "media-3", "media-4", "media-5", "media-6",
                "media-7", "media-8", "media-9", "media-10", "media-11",
            ];
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            GRIDS
                .iter()
                .filter_map(|g| cx.debug_bounds(g))
                .map(|b| f32::from(b.size.height))
                .collect::<Vec<_>>()
        };
        let uncapped = grids(cx);
        assert!(uncapped.iter().any(|h| *h > 160.), "some media are taller: {uncapped:?}");

        // Dragging the slider shows the cap at once, but doesn't save it yet.
        let cap = Some(MaxHeight::new(150.));
        app.update(cx, |app, cx| app.set_media_max_height(cap, false, cx));
        let capped = grids(cx);
        assert_eq!(capped.len(), uncapped.len(), "the same rows are drawn");
        // The grid's 1px border is outside the cap.
        assert!(capped.iter().all(|h| *h <= 152.5), "capped at 150px: {capped:?}");
        assert_eq!(Settings::load(&path).unwrap().media_max_height, None);

        // Releasing it saves; the last step lifts the cap again.
        app.update(cx, |app, cx| app.set_media_max_height(cap, true, cx));
        cx.run_until_parked();
        assert_eq!(Settings::load(&path).unwrap().media_max_height, cap);
        app.update(cx, |app, cx| app.set_media_max_height(None, true, cx));
        cx.run_until_parked();
        assert_eq!(grids(cx), uncapped);
        assert_eq!(Settings::load(&path).unwrap().media_max_height, None);
    }

    #[gpui_kit::test]
    fn column_settings_apply_live_and_are_saved(cx: &mut TestAppContext) {
        use crate::state::deck::ColumnWidth;
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(theme::NAMES[0], cx);
            keys::bind(cx);
            cx.set_global(Runtime::inline());
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decks.json");
        let options = AppOptions { deck_path: Some(path.clone()), ..AppOptions::default() };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), Settings::default(), backend, options, window, cx)
        });
        cx.run_until_parked();
        let column = app.read_with(cx, |app, _| app.columns().next().cloned()).unwrap();
        let id = column.read_with(cx, |c, _| c.id());

        // The header's settings button.
        column.update(cx, |_, cx| cx.emit(ColumnEvent::OpenSettings(id)));
        cx.run_until_parked();
        let dialog = app.read_with(cx, |app, _| app.column_settings_dialog().cloned());
        let dialog = dialog.expect("the dialog opened");
        assert_eq!(dialog.read_with(cx, |d, _| d.id()), id);

        // Dragging the slider resizes the column without saving.
        dialog.update(cx, |d, cx| d.set_width(500., false, cx));
        cx.run_until_parked();
        assert_eq!(column.read_with(cx, |c, _| c.settings().width.px()), 500.);
        assert!(!path.exists(), "nothing saved while dragging");

        dialog.update(cx, |d, cx| {
            d.set_width(480., true, cx);
            d.set_name("Following only", cx);
        });
        cx.run_until_parked();
        assert_eq!(column.read_with(cx, |c, _| c.title()), "Following only");
        let saved = DeckFile::load(&path).unwrap();
        let config = saved.active().columns.iter().find(|c| c.id == id).cloned().unwrap();
        assert_eq!(config.settings.width, ColumnWidth::new(480.));
        assert_eq!(config.settings.name.as_deref(), Some("Following only"));
        assert_eq!(app.read_with(cx, |app, _| app.deck().clone()), saved);

        // Esc closes it, even from the focused name field, and saves a change
        // that was only previewed (a slider moved by keyboard).
        dialog.update(cx, |d, cx| d.set_width(560., false, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| app.column_settings_dialog().is_none()));
        let saved = DeckFile::load(&path).unwrap();
        let config = saved.active().columns.iter().find(|c| c.id == id).cloned().unwrap();
        assert_eq!(config.settings.width, ColumnWidth::new(560.));
    }

    /// Releases served from memory; counts checks and records launches.
    #[derive(Default)]
    struct FakeReleases {
        latest: std::sync::Mutex<Option<Release>>,
        /// When set, downloads wait until it fires.
        gate: std::sync::Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
        checks: std::sync::atomic::AtomicUsize,
        launched: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl FakeReleases {
        fn serving(version: Version) -> Arc<Self> {
            let release = Release::from_assets(
                &format!("v{version}"),
                "https://example.test/notes",
                vec![crate::state::update::Asset {
                    name: crate::state::update::installer_name(version),
                    url: "https://example.test/setup.exe".into(),
                    sha256: crate::state::update::Sha256::from_hex(&"ab".repeat(32)),
                }],
            )
            .unwrap();
            Arc::new(Self { latest: std::sync::Mutex::new(Some(release)), ..Self::default() })
        }

        fn checks(&self) -> usize {
            self.checks.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::update::ReleaseSource for FakeReleases {
        fn latest(&self) -> twitty_model::BoxFuture<'_, Result<Option<Release>, UpdateError>> {
            self.checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let latest = self.latest.lock().unwrap().clone();
            Box::pin(async move { Ok(latest) })
        }

        fn download<'a>(
            &'a self,
            release: &'a Release,
        ) -> twitty_model::BoxFuture<'a, Result<PathBuf, UpdateError>> {
            let path = PathBuf::from("downloads").join(&release.installer.name);
            let gate = self.gate.lock().unwrap().take();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.await.ok();
                }
                Ok(path)
            })
        }
    }

    fn update_services(releases: &Arc<FakeReleases>, launch_works: bool) -> UpdateServices {
        let launched = Arc::clone(releases);
        UpdateServices {
            source: Arc::clone(releases) as Arc<dyn crate::update::ReleaseSource>,
            launch: Arc::new(move |path: &std::path::Path| {
                if !launch_works {
                    return Err(UpdateError::Launch("access denied".into()));
                }
                launched.launched.lock().unwrap().push(path.to_path_buf());
                Ok(())
            }),
        }
    }

    fn start_with_updates(
        cx: &mut TestAppContext,
        services: UpdateServices,
        settings: Settings,
    ) -> (Entity<TwittyApp>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(theme::NAMES[0], cx);
            cx.set_global(Runtime::inline());
        });
        let options = AppOptions {
            session: Some(SessionSource::Keyring),
            updates: Some(services),
            ..AppOptions::default()
        };
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let (app, cx) = cx.add_window_view(|window, cx| {
            TwittyApp::new(DeckFile::default(), settings, backend, options, window, cx)
        });
        cx.run_until_parked();
        (app, cx)
    }

    fn newer() -> Version {
        let current = Version::current();
        Version { minor: current.minor + 1, ..current }
    }

    #[gpui_kit::test]
    fn a_newer_release_is_offered_shortly_after_startup(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(newer());
        let (app, cx) =
            start_with_updates(cx, update_services(&releases, true), Settings::default());
        assert_eq!(releases.checks(), 0, "not while the app starts");

        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();
        assert_eq!(releases.checks(), 1);
        let status = app.read_with(cx, |app, _| app.updates().status().clone());
        assert!(
            matches!(&status, UpdateStatus::Available(r) if r.version == newer()),
            "{status:?}"
        );
        assert!(cx.debug_bounds("update-banner").is_some(), "the banner shows");

        app.update(cx, |app, cx| app.dismiss_update(cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("update-banner").is_none(), "Later hides it");

        cx.executor().advance_clock(UPDATE_INTERVAL);
        cx.run_until_parked();
        assert_eq!(releases.checks(), 2, "checked again later");
        assert!(cx.debug_bounds("update-banner").is_none(), "without offering it again");
    }

    #[gpui_kit::test]
    fn the_current_version_is_not_offered(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(Version::current());
        let (app, cx) =
            start_with_updates(cx, update_services(&releases, true), Settings::default());
        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();
        assert_eq!(releases.checks(), 1);
        assert_eq!(app.read_with(cx, |app, _| app.updates().status().clone()), UpdateStatus::Idle);
        assert!(cx.debug_bounds("update-banner").is_none());

        // "Check now" says so.
        app.update(cx, |app, cx| app.check_for_updates(cx));
        cx.run_until_parked();
        let toasts = app.read_with(cx, |app, _| {
            app.toasts().items().iter().map(|t| t.text.clone()).collect::<Vec<_>>()
        });
        assert_eq!(toasts, vec![format!("Twitty {} is up to date", Version::current())]);
    }

    #[gpui_kit::test]
    fn turning_the_setting_off_stops_the_checks(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(newer());
        let off = Settings { check_updates: Some(false), ..Settings::default() };
        let (app, cx) = start_with_updates(cx, update_services(&releases, true), off);
        cx.executor().advance_clock(UPDATE_INTERVAL);
        cx.run_until_parked();
        assert_eq!(releases.checks(), 0, "off: never checked");

        app.update(cx, |app, cx| app.set_check_updates(true, cx));
        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();
        assert_eq!(releases.checks(), 1, "on again: checked");
    }

    #[gpui_kit::test]
    fn install_downloads_and_starts_the_installer(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(newer());
        let (app, cx) =
            start_with_updates(cx, update_services(&releases, true), Settings::default());
        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();

        app.update(cx, |app, cx| app.install_update(cx));
        cx.run_until_parked();
        let expected =
            PathBuf::from("downloads").join(crate::state::update::installer_name(newer()));
        assert_eq!(*releases.launched.lock().unwrap(), vec![expected]);
    }

    #[gpui_kit::test]
    fn check_now_during_a_download_does_not_cancel_it(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(newer());
        let (release_download, gate) = futures::channel::oneshot::channel();
        *releases.gate.lock().unwrap() = Some(gate);
        let (app, cx) =
            start_with_updates(cx, update_services(&releases, true), Settings::default());
        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();

        app.update(cx, |app, cx| app.install_update(cx));
        cx.run_until_parked();
        let status = app.read_with(cx, |app, _| app.updates().status().clone());
        assert!(matches!(status, UpdateStatus::Downloading(_)), "{status:?}");

        app.update(cx, |app, cx| app.check_for_updates(cx));
        cx.run_until_parked();
        release_download.send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(releases.launched.lock().unwrap().len(), 1, "the installer still started");
    }

    #[gpui_kit::test]
    fn a_failed_install_can_be_retried(cx: &mut TestAppContext) {
        let releases = FakeReleases::serving(newer());
        let (app, cx) =
            start_with_updates(cx, update_services(&releases, false), Settings::default());
        cx.executor().advance_clock(UPDATE_FIRST_CHECK);
        cx.run_until_parked();

        app.update(cx, |app, cx| app.install_update(cx));
        cx.run_until_parked();
        let status = app.read_with(cx, |app, _| app.updates().status().clone());
        assert!(
            matches!(&status, UpdateStatus::Failed(_, why) if why.contains("access denied")),
            "{status:?}"
        );
        assert!(cx.debug_bounds("update-banner").is_some(), "the banner says why, with Retry");
    }
}
