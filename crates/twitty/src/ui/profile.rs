//! The profile sheet: a side panel with an account's header and its Tweets
//! and Media tabs.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, ListAlignment, ListState,
    ObjectFit, ParentElement, Render, Styled, Subscription, Task, Window, assets::IconName,
    component::button::*, component::scroll::ScrollableElement, component::spinner::Spinner,
    component::*, div, img, list, prelude::*, px,
};
use twitty_model::{Backend, BackendError, FeedPage, Media, Profile};

use crate::{
    format::compact_count,
    runtime::Runtime,
    state::{
        column::{ColumnModel, ListChange, LoadKind, Status},
        profile::{self, ProfileTab, Row, Tile},
    },
    ui::button::TextButton,
    ui::{
        hub::{InteractionsHub, Navigate},
        post::{RowCtx, avatar, fmt_duration, render_item, rich_text, verified_badge, video_badge},
        remote_image,
        thread::PANEL_WIDTH,
    },
};

/// Start loading older rows when this close to the bottom.
const PREFETCH_ROWS: usize = 5;
/// Space between gallery tiles.
const TILE_GAP: f32 = 2.0;
/// The banner spans the panel (inside its 1px left border), at X's 3:1.
const BANNER_W: f32 = PANEL_WIDTH - 1.0;
const BANNER_H: f32 = BANNER_W / 3.0;
/// Diameter of the profile's avatar.
const AVATAR: f32 = 72.0;
/// Width of the ring that sets the avatar off the banner.
const AVATAR_RING: f32 = 4.0;
/// Side of a square gallery tile (the panel has a 1px left border).
const TILE: f32 = (PANEL_WIDTH - 1.0 - TILE_GAP * (profile::GALLERY_COLUMNS as f32 - 1.0))
    / profile::GALLERY_COLUMNS as f32;

/// Emitted when the user closes the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseProfile;

impl EventEmitter<CloseProfile> for ProfilePanel {}

/// An account's profile, posts and media.
pub struct ProfilePanel {
    profile: Arc<Profile>,
    tab: ProfileTab,
    tweets: ColumnModel,
    media: ColumnModel,
    /// The Media tab's items as gallery tiles.
    tiles: Vec<Tile>,
    list: ListState,
    backend: Arc<dyn Backend>,
    hub: Entity<InteractionsHub>,
    loads: [Option<Task<()>>; 2],
    _refresh: Option<Task<()>>,
    _hub_sub: Subscription,
}

impl std::fmt::Debug for ProfilePanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfilePanel")
            .field("user", &self.profile.id)
            .field("tab", &self.tab)
            .finish_non_exhaustive()
    }
}

impl ProfilePanel {
    /// Shows `profile` at once, then refreshes it and loads its posts.
    pub fn new(
        profile: Arc<Profile>,
        backend: Arc<dyn Backend>,
        hub: Entity<InteractionsHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let tab = ProfileTab::Tweets;
        let list = ListState::new(profile::row_count(tab, 0), ListAlignment::Top, px(600.));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            // GPUI calls this while the ListState is mutably borrowed: react after.
            let end = event.visible_range.end;
            let weak = weak.clone();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| this.on_scroll(end, cx)).ok();
            });
        });
        let mut this = Self {
            tweets: ColumnModel::new(ProfileTab::Tweets.source(&profile)),
            media: ColumnModel::new(ProfileTab::Media.source(&profile)),
            profile,
            tab,
            tiles: Vec::new(),
            list,
            _hub_sub: cx.observe(&hub, |_, _, cx| cx.notify()),
            backend,
            hub,
            loads: [None, None],
            _refresh: None,
        };
        this.refresh_profile(cx);
        this.load(tab, LoadKind::Refresh, cx);
        this
    }

    pub fn profile(&self) -> &Arc<Profile> {
        &self.profile
    }

    pub fn tab(&self) -> ProfileTab {
        self.tab
    }

    pub fn model(&self, tab: ProfileTab) -> &ColumnModel {
        match tab {
            ProfileTab::Tweets => &self.tweets,
            ProfileTab::Media => &self.media,
        }
    }

    fn model_mut(&mut self, tab: ProfileTab) -> &mut ColumnModel {
        match tab {
            ProfileTab::Tweets => &mut self.tweets,
            ProfileTab::Media => &mut self.media,
        }
    }

    /// The gallery of the Media tab.
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Items (Tweets) or tiles (Media) shown by `tab`.
    fn content(&self, tab: ProfileTab) -> usize {
        match tab {
            ProfileTab::Tweets => self.tweets.items().len(),
            ProfileTab::Media => self.tiles.len(),
        }
    }

    /// The profile in a post may lack its bio or counts: fetch it whole.
    fn refresh_profile(&mut self, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let handle = self.profile.handle.to_string();
        let result = Runtime::global(cx).spawn(async move { backend.profile(&handle).await });
        self._refresh = Some(cx.spawn(async move |this, cx| {
            match result.await {
                Ok(Ok(fresh)) => {
                    this.update(cx, |this, cx| {
                        if fresh.id == this.profile.id {
                            this.profile = Arc::new(fresh);
                            // The header's height may change.
                            this.list.splice(0..1, 1);
                            cx.notify();
                        }
                    })
                    .ok();
                }
                Ok(Err(e)) => tracing::warn!("profile refresh failed: {e}"),
                Err(e) => tracing::warn!("profile refresh failed: {e}"),
            }
        }));
    }

    /// Shows `tab`, loading it the first time.
    pub fn switch_tab(&mut self, tab: ProfileTab, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        self.list.reset(profile::row_count(tab, self.content(tab)));
        if !self.model(tab).loaded_once() {
            self.load(tab, LoadKind::Refresh, cx);
        }
        cx.notify();
    }

    /// Starts a load of `tab` unless one is already running.
    pub fn load(&mut self, tab: ProfileTab, kind: LoadKind, cx: &mut Context<Self>) {
        let Some((kind, request)) = self.model_mut(tab).begin(kind, chrono::Utc::now()) else {
            return;
        };
        let backend = Arc::clone(&self.backend);
        let source = self.model(tab).source().clone();
        let result = Runtime::global(cx).spawn(async move { backend.feed(&source, request).await });
        self.loads[tab as usize] = Some(cx.spawn(async move |this, cx| {
            let result = match result.await {
                Ok(r) => r,
                Err(e) => Err(BackendError::Network(e.to_string())),
            };
            this.update(cx, |this, cx| this.apply(tab, kind, result, cx)).ok();
        }));
        cx.notify();
    }

    fn apply(
        &mut self,
        tab: ProfileTab,
        kind: LoadKind,
        result: Result<FeedPage, BackendError>,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = &result {
            tracing::warn!(?tab, ?kind, "profile load failed: {e}");
        }
        let old = self.content(tab);
        let change = self.model_mut(tab).finish(kind, result);
        if tab == ProfileTab::Media {
            self.tiles = profile::tiles(self.media.items());
        }
        // Rows of the content left as they were.
        let kept = match &change {
            ListChange::None => None,
            ListChange::Reset { .. } => Some(0),
            ListChange::Splice { range, .. } => Some(match tab {
                ProfileTab::Tweets => range.start,
                ProfileTab::Media => profile::tiles(&self.media.items()[..range.start]).len(),
            }),
        };
        if tab == self.tab {
            match kept {
                Some(0) => self.list.reset(profile::row_count(tab, self.content(tab))),
                Some(kept) => {
                    let (range, count) = profile::splice(tab, kept, old, self.content(tab));
                    self.list.splice(range, count);
                }
                // The footer may show a spinner or an error now.
                None => {
                    let footer = profile::row_count(tab, old) - 1;
                    self.list.splice(footer..footer + 1, 1);
                }
            }
        }
        cx.notify();
    }

    fn on_scroll(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        let tab = self.tab;
        let model = self.model(tab);
        let rows = profile::row_count(tab, self.content(tab));
        if model.loaded_once() && model.has_older() && visible_end + PREFETCH_ROWS >= rows {
            self.load(tab, LoadKind::Older, cx);
        }
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match profile::row(self.tab, ix, self.content(self.tab)) {
            Row::Header => self.render_header(window, cx).into_any_element(),
            Row::Tabs => self.render_tabs(cx).into_any_element(),
            Row::Item(i) => match self.tweets.items().get(i) {
                Some(item) => {
                    let rcx = RowCtx {
                        ix: i,
                        column_width: PANEL_WIDTH,
                        hub: self.hub.clone(),
                        scope: "profile".into(),
                    };
                    render_item(item, &rcx, window, cx)
                }
                None => div().into_any_element(),
            },
            Row::Gallery(range) => self.render_gallery_row(range).into_any_element(),
            Row::Footer => self.render_footer(cx),
        }
    }

    fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let p = &self.profile;
        let count = |n: u64, label: &'static str| {
            h_flex()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(compact_count(n)),
                )
                .child(label)
        };
        // X's banners are 3:1. The avatar overlaps the banner's bottom edge,
        // ringed in the background colour.
        let banner = p.banner.as_ref().map(|b| {
            img(remote_image::source(b.url.as_str(), BANNER_W, BANNER_H))
                .debug_selector(|| "profile-banner".into())
                .w(px(BANNER_W))
                .h(px(BANNER_H))
                .aspect_ratio(BANNER_W / BANNER_H)
                .object_fit(ObjectFit::Cover)
                .bg(theme.muted)
        });
        let face = div()
            .debug_selector(|| "profile-avatar".into())
            .flex_none()
            .rounded_full()
            .when(banner.is_some(), |d| {
                d.border(px(AVATAR_RING))
                    .border_color(theme.background)
                    .bg(theme.background)
                    .mt(px(-(AVATAR / 2. + AVATAR_RING)))
            })
            .child(avatar(p, AVATAR));
        v_flex().w_full().children(banner).child(
            v_flex()
                .w_full()
                .px_4()
                .when(p.banner.is_none(), |d| d.pt_4())
                .pb_3()
                .gap_3()
                .child(h_flex().child(face))
                .child(
                    v_flex()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .child(p.display_name.clone()),
                                )
                                .children(p.badges.verified.map(|v| verified_badge(v, &theme))),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(format!("@{}", p.handle)),
                        ),
                )
                .when(!p.bio.is_empty(), |d| {
                    d.child(rich_text(("bio", 0), &p.bio, &self.hub, theme.link, window, cx))
                })
                .child(
                    h_flex()
                        .debug_selector(|| "profile-counts".into())
                        .gap_4()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(count(p.counts.posts, "Tweets"))
                        .child(count(p.counts.following, "Following"))
                        .child(count(p.counts.followers, "Followers")),
                ),
        )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.tab;
        let tab = |tab: ProfileTab| {
            let active = tab == current;
            v_flex()
                .id(tab.label())
                .debug_selector(move || format!("profile-tab-{}", tab.label()))
                .flex_1()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.switch_tab(tab, cx)))
                .child(
                    div()
                        .py_3()
                        .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                        .text_color(if active { theme.foreground } else { theme.muted_foreground })
                        .child(tab.label()),
                )
                .child(
                    div()
                        .h(px(4.))
                        .w(px(56.))
                        .rounded(px(2.))
                        .when(active, |d| d.bg(theme.primary)),
                )
        };
        // A list row only gets its content's width unless it asks for more.
        h_flex()
            .w_full()
            .border_b_1()
            .border_color(theme.border)
            .children(ProfileTab::ALL.into_iter().map(tab))
    }

    fn render_gallery_row(&self, range: std::ops::Range<usize>) -> impl IntoElement {
        let start = range.start;
        h_flex().gap(px(TILE_GAP)).pb(px(TILE_GAP)).children(
            self.tiles[range].iter().enumerate().map(|(k, Tile { post, index })| {
                let ix = start + k;
                let media = &post.media[*index];
                let badge = match media {
                    Media::Video { duration, .. } => {
                        Some(video_badge("▶", duration.map(fmt_duration)))
                    }
                    Media::Gif { .. } => Some(video_badge("GIF", None)),
                    Media::Image { .. } => None,
                };
                let hub = self.hub.clone();
                let to = Navigate::Media { post: Arc::clone(post), index: *index };
                div()
                    .id(("gallery-tile", ix))
                    .debug_selector(move || format!("gallery-tile-{ix}"))
                    .relative()
                    .flex_none()
                    .size(px(TILE))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let to = to.clone();
                        hub.update(cx, |hub, cx| hub.navigate(to, cx));
                    })
                    .child(
                        img(remote_image::source(media.preview().url.as_str(), TILE, TILE))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .children(badge)
            }),
        )
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let tab = self.tab;
        let model = self.model(tab);
        let body: AnyElement = match model.status() {
            Status::Loading(_) => Spinner::new().into_any_element(),
            Status::Failed { error, .. } => {
                let retry = model.retry_kind().unwrap_or(LoadKind::Refresh);
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(theme.muted_foreground).child(error.to_string()))
                    .child(
                        Button::new("profile-retry")
                            .text_button(Size::Small)
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| this.load(tab, retry, cx))),
                    )
                    .into_any_element()
            }
            Status::Idle if model.loaded_once() && self.content(tab) == 0 => div()
                .text_color(theme.muted_foreground)
                .child(match tab {
                    ProfileTab::Tweets => "No posts yet",
                    ProfileTab::Media => "No photos or videos yet",
                })
                .into_any_element(),
            Status::Idle => div().into_any_element(),
        };
        h_flex().w_full().justify_center().py_6().child(body).into_any_element()
    }
}

impl Render for ProfilePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let p = Arc::clone(&self.profile);
        let hub = self.hub.clone();
        v_flex()
            .w(px(PANEL_WIDTH))
            .h_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(53.))
                    .px_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Icon::new(IconName::User).size(px(20.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_lg()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(self.profile.display_name.clone()),
                    )
                    .child(
                        Button::new("profile-add-column")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("Add as a column")
                            .on_click(move |_, _, cx| {
                                let to = Navigate::UserColumn(Arc::clone(&p));
                                hub.update(cx, |hub, cx| hub.navigate(to, cx));
                            }),
                    )
                    .child(
                        Button::new("profile-close")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseProfile))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list(self.list.clone(), cx.processor(Self::render_row)).size_full())
                    .vertical_scrollbar(&self.list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use twitty_model::{ImageRef, UserId, mock::MockBackend};

    /// Draws the panel for `profile`, unknown to the mock so the refresh
    /// keeps it as given.
    fn header(profile: Profile, cx: &mut TestAppContext) -> &mut gpui_kit::VisualTestContext {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Runtime::inline());
        });
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&backend)));
        let (_panel, cx) =
            cx.add_window_view(|_, cx| ProfilePanel::new(Arc::new(profile), backend, hub, cx));
        cx.run_until_parked();
        cx
    }

    fn someone() -> Profile {
        Profile::new(UserId::new("1"), "nobody_in_the_mock", "Someone")
    }

    #[gpui_kit::test]
    fn the_banner_spans_the_header_under_the_avatar(cx: &mut TestAppContext) {
        let mut profile = someone();
        let banner: ImageRef = serde_json::from_value(serde_json::json!({
            "url": "https://pbs.twimg.com/profile_banners/1/2",
            "size": null,
        }))
        .unwrap();
        profile.banner = Some(banner);
        let cx = header(profile, cx);

        let banner = cx.debug_bounds("profile-banner").expect("the banner is drawn");
        assert!((banner.size.width - px(BANNER_W)).abs() < px(0.5), "{banner:?}");
        assert!((banner.size.height - px(BANNER_H)).abs() < px(0.5), "3:1: {banner:?}");
        let face = cx.debug_bounds("profile-avatar").expect("the avatar is drawn");
        assert!(face.top() < banner.bottom() && face.bottom() > banner.bottom(), "{face:?}");
    }

    #[gpui_kit::test]
    fn without_a_banner_the_avatar_leads(cx: &mut TestAppContext) {
        let cx = header(someone(), cx);
        assert!(cx.debug_bounds("profile-banner").is_none());
        let face = cx.debug_bounds("profile-avatar").expect("the avatar is drawn");
        let counts = cx.debug_bounds("profile-counts").expect("the counts are drawn");
        assert!(face.top() > px(53.) && face.bottom() < counts.top(), "{face:?}");
    }
}
