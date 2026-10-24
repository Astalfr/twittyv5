//! Rendering of feed rows: posts, reposts, threads and their embeds.

use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    rc::Rc,
    sync::Arc,
};

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, ClipboardItem, ElementId, Entity,
    FontWeight, HighlightStyle, Hsla, ImageSource, Img, InteractiveText, IntoElement, ObjectFit,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    StyledText, UnderlineStyle, Window,
    component::avatar::Avatar,
    component::button::*,
    component::hover_card::HoverCard,
    component::menu::{ContextMenuExt, PopupMenuItem},
    component::tooltip::Tooltip,
    component::*,
    div, img,
    prelude::*,
    px, rems,
};
use twitty_model::{
    Embed, FeedItem, LinkCard, Media, NotificationKind, Poll, Post, PostOrGone, Profile, RichText,
    Span, SpanKind, UnavailableReason, VerifiedKind,
};

use crate::{
    format::{compact_count, exact_count, exact_time, relative_time},
    state::{interactions::Toggle, links, media_layout, motion, toasts::Notice},
    ui::button::TextButton,
    ui::{
        assets::{BOOKMARK_FILL, HEART_FILL},
        gif::inline_gif,
        hub::{InteractionsHub, Navigate},
        remote_image,
    },
};

/// Horizontal padding of a row.
const PAD_X: f32 = 16.0;
/// Avatar diameter.
const AVATAR: f32 = 40.0;
/// Gap between avatar and content.
const GAP: f32 = 12.0;

/// Width available to post content (text, media) in a column of `column_width`.
pub fn content_width(column_width: f32) -> f32 {
    (column_width - 2.0 * PAD_X - AVATAR - GAP).max(0.0)
}

/// Everything a row needs besides the item itself.
#[derive(Debug, Clone)]
pub struct RowCtx {
    pub ix: usize,
    pub column_width: f32,
    pub hub: Entity<InteractionsHub>,
    /// Unique to the list the row is in (a column, the thread panel, …), so
    /// state kept per row (a playing GIF) isn't shared with the same post
    /// shown in another list.
    pub scope: ElementId,
}

/// Renders one feed row.
pub fn render_item(item: &FeedItem, rcx: &RowCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let row = div().w_full().border_b_1().border_color(theme.border);
    match item {
        FeedItem::Post(post) => {
            row.child(post_row(post, None, Chain::Alone, rcx, 0, window, cx)).into_any_element()
        }
        FeedItem::Repost { by, post } => {
            row.child(post_row(post, Some(by), Chain::Alone, rcx, 0, window, cx)).into_any_element()
        }
        FeedItem::Thread(posts) => row
            .children(
                posts
                    .iter()
                    .enumerate()
                    .map(|(k, p)| post_row(p, None, Chain::at(k, posts.len()), rcx, k, window, cx)),
            )
            .into_any_element(),
        FeedItem::Notification(n) => row
            .child(
                h_flex()
                    .px(px(PAD_X))
                    .py_3()
                    .gap(px(GAP))
                    .items_start()
                    .child(
                        div().w(px(AVATAR)).flex().justify_end().child(
                            Icon::new(notification_icon(n.kind))
                                .text_color(notification_color(n.kind, &theme))
                                .size(px(26.)),
                        ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_1()
                                    .children(n.actors.iter().take(6).enumerate().map(|(k, a)| {
                                        let key = rcx.ix * 8 + k;
                                        div()
                                            .id(("notif-actor", key))
                                            .debug_selector(move || format!("notif-actor-{key}"))
                                            .cursor_pointer()
                                            .on_click(open_profile(a, &rcx.hub))
                                            .child(avatar(a, 32.0))
                                    }))
                                    .child({
                                        let ix = rcx.ix;
                                        let exact = exact_time(n.created_at, &chrono::Local);
                                        div()
                                            .id(("notif-time", ix))
                                            .debug_selector(move || format!("notif-time-{ix}"))
                                            .flex_none()
                                            .ml_auto()
                                            .pl_1()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .whitespace_nowrap()
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(exact.clone()).build(window, cx)
                                            })
                                            .child(relative_time(n.created_at, Utc::now()))
                                    }),
                            )
                            .child(rich_text(
                                ("notif", rcx.ix),
                                &n.text,
                                &rcx.hub,
                                theme.link,
                                window,
                                cx,
                            ))
                            .children(n.target.as_ref().filter(|p| !p.body.is_empty()).map(|p| {
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(p.body.text().to_owned())
                            })),
                    ),
            )
            .into_any_element(),
        FeedItem::Unavailable { reason, .. } => row
            .child(notice(unavailable_text(reason), &theme.muted, theme.muted_foreground))
            .into_any_element(),
        FeedItem::Unsupported { reason, .. } => row
            .child(notice(
                format!("Twitty can't show this item yet ({reason})."),
                &theme.muted,
                theme.muted_foreground,
            ))
            .into_any_element(),
    }
}

fn notification_icon(kind: NotificationKind) -> IconName {
    match kind {
        NotificationKind::Like => IconName::Heart,
        NotificationKind::Repost => IconName::Repeat2,
        NotificationKind::Follow => IconName::User,
        NotificationKind::Quote => IconName::MessageCircle,
        NotificationKind::Other => IconName::Bell,
    }
}

fn notification_color(kind: NotificationKind, theme: &gpui_kit::component::Theme) -> Hsla {
    match kind {
        NotificationKind::Like => theme.red,
        NotificationKind::Repost => theme.green,
        NotificationKind::Follow | NotificationKind::Quote | NotificationKind::Other => theme.blue,
    }
}

fn notice(text: impl Into<SharedString>, bg: &Hsla, fg: Hsla) -> impl IntoElement {
    div().px(px(PAD_X)).py_3().child(
        div().px_3().py_2().rounded(px(12.)).bg(*bg).text_color(fg).text_sm().child(text.into()),
    )
}

fn unavailable_text(reason: &UnavailableReason) -> String {
    match reason {
        UnavailableReason::Deleted => "This post was deleted by the author.".into(),
        UnavailableReason::Suspended => "This post is from a suspended account.".into(),
        UnavailableReason::Protected => {
            "You're unable to view this post because the account owner limits who can view their posts.".into()
        }
        UnavailableReason::Withheld => "This post is withheld in your country.".into(),
        UnavailableReason::Other(Some(msg)) => msg.clone(),
        UnavailableReason::Other(None) => "This post is unavailable.".into(),
    }
}

pub(crate) fn avatar(profile: &Profile, size: f32) -> impl IntoElement + use<> {
    let mut a = Avatar::new().name(profile.display_name.clone()).size(px(size));
    if let Some(src) = &profile.avatar {
        a = a.src(src.url.as_str().to_owned());
    }
    a
}

/// Handle size relative to the name's (`text_sm`).
const HANDLE_SCALE: f32 = 0.875;

/// How far above the name's baseline the handle sits: a little higher
/// reads better next to the bold name.
const HANDLE_RAISE: f32 = 1.0;

/// Distance from the bottom of a one-line text box to its baseline: GPUI
/// centres the glyphs' ascent + descent in the line height.
fn baseline_from_bottom(line_height: Pixels, ascent: Pixels, descent: Pixels) -> Pixels {
    (line_height - ascent + descent) / 2.
}

/// How far to lift the (smaller) handle off the bottom of the header so it
/// sits `HANDLE_RAISE` above the bold name's baseline. GPUI's flex layout has no baseline
/// alignment, so this measures the fonts.
fn handle_lift(window: &Window, family: &SharedString) -> Pixels {
    let rem = window.rem_size();
    let base = window.text_style();
    let text_system = window.text_system();
    let gap = |weight: FontWeight, size: Pixels| {
        let mut style = base.clone();
        style.font_family = family.clone();
        style.font_weight = weight;
        style.font_size = size.into();
        let font = text_system.resolve_font(&style.font());
        baseline_from_bottom(
            // Unrounded, like the laid-out text box.
            style.line_height.to_pixels(style.font_size, rem),
            text_system.ascent(font, size),
            text_system.descent(font, size),
        )
    };
    let lift = gap(FontWeight::BOLD, rem) - gap(FontWeight::NORMAL, rem * HANDLE_SCALE);
    (lift + px(HANDLE_RAISE)).max(px(0.))
}

/// A click handler that opens `profile` (instead of the post's thread).
fn open_profile(
    profile: &Arc<Profile>,
    hub: &Entity<InteractionsHub>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let profile = Arc::clone(profile);
    let hub = hub.clone();
    move |_, _, cx| {
        cx.stop_propagation();
        let to = Navigate::Profile(Arc::clone(&profile));
        hub.update(cx, |hub, cx| hub.navigate(to, cx));
    }
}

/// An avatar that opens the account's profile on click and shows its card
/// on hover, with a button to add a column of its posts.
fn profile_hover(
    key: usize,
    profile: &Arc<Profile>,
    hub: &Entity<InteractionsHub>,
) -> impl IntoElement {
    let trigger = div()
        .id(("author-avatar", key))
        .debug_selector(|| format!("post-avatar-{key}"))
        .cursor_pointer()
        .on_click(open_profile(profile, hub))
        .child(avatar(profile, AVATAR));
    let profile = Arc::clone(profile);
    let hub = hub.clone();
    HoverCard::new(("author", key)).trigger(trigger).content(move |_, _, cx| {
        let theme = cx.theme().clone();
        let p = Arc::clone(&profile);
        let hub = hub.clone();
        v_flex()
            .w(px(300.))
            .gap_2()
            .p_1()
            .child(avatar(&p, 56.0))
            .child(
                v_flex()
                    .child(div().font_weight(FontWeight::BOLD).child(p.display_name.clone()))
                    .child(
                        div().text_color(theme.muted_foreground).child(format!("@{}", p.handle)),
                    ),
            )
            .when(!p.bio.is_empty(), |d| d.child(div().text_sm().child(p.bio.text().to_owned())))
            .child(
                h_flex()
                    .gap_3()
                    .text_sm()
                    .child(format!("{} Following", compact_count(p.counts.following)))
                    .child(format!("{} Followers", compact_count(p.counts.followers))),
            )
            .child(
                Button::new("open-profile").text_button(Size::Small).label("Open column").on_click(
                    move |_, _, cx| {
                        let to = Navigate::UserColumn(Arc::clone(&p));
                        hub.update(cx, |hub, cx| hub.navigate(to, cx));
                    },
                ),
            )
    })
}

/// Where a post sits in a chain of connected posts (a thread), which decides
/// the line drawn through its avatar column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    Alone,
    First,
    Middle,
    Last,
}

impl Chain {
    /// The position of post `k` of `len`.
    pub(crate) fn at(k: usize, len: usize) -> Self {
        match (k, len) {
            (_, 0 | 1) => Chain::Alone,
            (0, _) => Chain::First,
            (k, len) if k + 1 >= len => Chain::Last,
            _ => Chain::Middle,
        }
    }

    /// Whether a post above leads into this one.
    fn joined_above(self) -> bool {
        matches!(self, Chain::Middle | Chain::Last)
    }

    /// Whether this post leads into one below.
    fn joined_below(self) -> bool {
        matches!(self, Chain::First | Chain::Middle)
    }
}

/// Padding above a post's avatar and content.
const ROW_TOP: f32 = 12.0;
/// Space between the thread line and the avatar it leads into or out of.
const LINE_GAP: f32 = 4.0;
/// Width of the thread line.
const LINE_W: f32 = 2.0;

fn post_row(
    post: &Post,
    reposted_by: Option<&Arc<Profile>>,
    chain: Chain,
    rcx: &RowCtx,
    sub: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let hub = rcx.hub.read(cx);
    let eff = hub.effective(post);
    let pops: Vec<Toggle> = [Toggle::Like, Toggle::Repost, Toggle::Bookmark]
        .into_iter()
        .filter(|&t| hub.popping(&post.id, t))
        .collect();
    let width = content_width(rcx.column_width);
    let key = rcx.ix * 16 + sub;

    let link = links::vx_link(post);

    // A long name or handle truncates; the time never does, and sits at the
    // far right.
    let header = h_flex()
        .w_full()
        .gap_1()
        .min_w_0()
        .overflow_hidden()
        .text_color(theme.muted_foreground)
        .child(
            div()
                .id(("author-name", key))
                .debug_selector(|| format!("post-name-{key}"))
                .min_w_0()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .truncate()
                .hover(|s| s.underline())
                .on_click(open_profile(&post.author, &rcx.hub))
                .child(post.author.display_name.clone()),
        )
        .children(post.author.badges.verified.map(|v| verified_badge(v, &theme)))
        .child(
            div()
                .id(("author-handle", key))
                .debug_selector(|| format!("post-handle-{key}"))
                .min_w_0()
                .text_size(rems(HANDLE_SCALE))
                .self_end()
                .mb(handle_lift(window, &theme.font_family))
                .truncate()
                .on_click(open_profile(&post.author, &rcx.hub))
                .child(format!("@{}", post.author.handle)),
        )
        .child({
            let link = link.clone();
            let exact = exact_time(post.created_at, &chrono::Local);
            div()
                .id(("time", key))
                .debug_selector(|| format!("post-time-{key}"))
                .flex_none()
                .ml_auto()
                .pl_1()
                .text_sm()
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|s| s.underline())
                .tooltip(move |window, cx| Tooltip::new(exact.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&link);
                })
                .child(relative_time(post.created_at, Utc::now()))
        });

    let mut content = v_flex().flex_1().min_w_0().gap_1().child(header);

    if let Some(reply) = &post.reply_to
        && chain == Chain::Alone
        && let Some(handle) = &reply.handle
    {
        content = content.child(
            h_flex()
                .gap_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Replying to")
                .child(div().text_color(theme.link).child(format!("@{handle}"))),
        );
    }

    if !post.body.is_empty() {
        content =
            content.child(rich_text(("body", key), &post.body, &rcx.hub, theme.link, window, cx));
    }
    if !post.media.is_empty() {
        let grid = media_grid(("media", key), &post.media, width, post, rcx, &theme, window, cx);
        content = content.child(grid);
    }
    if let Some(embed) = &post.embed {
        content = content.child(render_embed(("embed", key), embed, width, &rcx.hub, window, cx));
    }
    content = content.child(action_bar(key, post, eff, &pops, rcx, &theme));

    // The row stretches the avatar column to its full height, so a thread's
    // line runs from the row's top edge into the avatar and from the avatar
    // to the row's bottom edge, meeting the next post's line without a break.
    let top = if reposted_by.is_some() { 0.0 } else { ROW_TOP };
    let line = || div().flex_none().w(px(LINE_W)).bg(theme.border);
    let avatar_col = v_flex()
        .items_center()
        .flex_none()
        .w(px(AVATAR))
        .child(div().flex_none().h(px(top)).when(chain.joined_above(), |d| {
            d.child(
                line().debug_selector(move || format!("line-above-{key}")).h(px(top - LINE_GAP)),
            )
        }))
        .child(profile_hover(key, &post.author, &rcx.hub))
        .when(chain.joined_below(), |col| {
            col.child(
                line()
                    .debug_selector(move || format!("line-below-{key}"))
                    .mt(px(LINE_GAP))
                    .flex_1(),
            )
        });
    content = content.pt(px(top)).pb_2();

    let hub = rcx.hub.clone();
    let menu_hub = rcx.hub.clone();
    let id = post.id.clone();
    v_flex()
        .id(("post", key))
        .debug_selector(move || format!("post-row-{key}"))
        .w_full()
        .px(px(PAD_X))
        .when(reposted_by.is_some(), |d| d.pt(px(ROW_TOP)))
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .on_click(move |_, _, cx| {
            hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(id.clone()), cx));
        })
        .when_some(reposted_by, |d, by| {
            d.child(
                h_flex()
                    .gap_2()
                    .pl(px(AVATAR - 16.0))
                    .pb_1()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Repeat2).size(px(14.)))
                    .child(
                        div()
                            .id(("reposter", key))
                            .debug_selector(|| format!("post-reposter-{key}"))
                            .hover(|s| s.underline())
                            .on_click(open_profile(by, &rcx.hub))
                            .child(format!("{} reposted", by.display_name)),
                    ),
            )
        })
        .child(h_flex().items_stretch().gap(px(GAP)).child(avatar_col).child(content))
        .context_menu(move |menu, _, _| {
            let link = link.clone();
            let hub = menu_hub.clone();
            menu.item(PopupMenuItem::new("Copy tweet link").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                hub.update(cx, |hub, cx| hub.notice(Notice::success("Link copied"), cx));
            }))
        })
        .into_any_element()
}

pub(crate) fn verified_badge(
    kind: VerifiedKind,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let color = match kind {
        VerifiedKind::Individual => theme.blue,
        VerifiedKind::Business => theme.yellow,
        VerifiedKind::Government => theme.muted_foreground,
    };
    Icon::new(IconName::BadgeCheck).size(px(16.)).text_color(color).flex_none()
}

/// Text with clickable mentions, hashtags and links. The hovered one is
/// underlined. A mention opens the account in the app, the others the browser.
pub(crate) fn rich_text(
    id: (&'static str, usize),
    text: &RichText,
    hub: &Entity<InteractionsHub>,
    link: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    // The span under the mouse; the text notifies the view when it changes.
    let hovered = window.use_keyed_state(
        ElementId::named_usize(format!("{}-hover", id.0), id.1),
        cx,
        |_, _| None::<usize>,
    );
    let hovered_span = *hovered.read(cx);
    let spans = text.spans().to_vec();
    let highlights = spans.iter().enumerate().map(|(ix, s)| {
        let underline = (hovered_span == Some(ix)).then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(link),
            wavy: false,
        });
        (s.range.clone(), HighlightStyle { color: Some(link), underline, ..Default::default() })
    });
    let styled =
        StyledText::new(SharedString::from(text.text().to_owned())).with_highlights(highlights);
    let ranges = spans.iter().map(|s| s.range.clone()).collect();
    let hover_spans = spans.clone();
    let on_text = hovered.clone();
    let hub = hub.clone();
    let text = InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| {
            let Some(span) = spans.get(ix) else { return };
            // A link click must not also open the post's thread.
            cx.stop_propagation();
            match &span.kind {
                SpanKind::Mention(user) => {
                    let to = Navigate::Mention(user.clone());
                    hub.update(cx, |hub, cx| hub.navigate(to, cx));
                }
                kind => cx.open_url(&span_url(kind)),
            }
        })
        .on_hover(move |byte, _, _, cx| {
            let span = byte.and_then(|b| span_at(&hover_spans, b));
            on_text.update(cx, |h, cx| {
                if *h != span {
                    *h = span;
                    cx.notify();
                }
            });
        });
    // The text only reports moves while the mouse is over it, so leaving it
    // straight from a link would keep the underline.
    div()
        .id(ElementId::named_usize(format!("{}-text", id.0), id.1))
        .debug_selector(move || format!("{}-text-{}", id.0, id.1))
        .w_full()
        .on_hover(move |inside, _, cx| {
            if !inside {
                hovered.update(cx, |h, cx| {
                    if h.take().is_some() {
                        cx.notify();
                    }
                });
            }
        })
        .child(text)
}

/// The index of the span covering byte `ix`, if any. `spans` must be sorted
/// and non-overlapping, as [`RichText`] guarantees.
pub fn span_at(spans: &[Span], ix: usize) -> Option<usize> {
    let i = spans.partition_point(|s| s.range.end <= ix);
    spans.get(i).filter(|s| s.range.contains(&ix)).map(|_| i)
}

/// The web page of a span, for the browser.
pub fn span_url(kind: &SpanKind) -> String {
    match kind {
        SpanKind::Mention(user) => format!("https://x.com/{}", user.handle),
        SpanKind::Hashtag { tag } => format!("https://x.com/hashtag/{tag}"),
        SpanKind::Cashtag { symbol } => format!("https://x.com/search?q=%24{symbol}"),
        SpanKind::Link { url } => url.to_string(),
    }
}

#[expect(clippy::too_many_arguments, reason = "the row's pieces, passed down as is")]
fn media_grid(
    id: (&'static str, usize),
    media: &[Media],
    width: f32,
    post: &Post,
    rcx: &RowCtx,
    theme: &gpui_kit::component::Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let hub = &rcx.hub;
    let post = Arc::new(post.clone());
    // Tiles are positioned inside the 1px border, so lay them out on the inner width.
    let inner = (width - 2.0 * BORDER).max(0.0);
    let grid =
        media_layout::layout(media.len(), media.first().and_then(|m| m.preview().aspect()), inner);
    div()
        .id(id)
        .relative()
        .mt_2()
        .w(px(width))
        .h(px(grid.height + 2.0 * BORDER))
        .rounded(px(RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .children(grid.tiles.iter().zip(media).enumerate().map(|(index, (tile, m))| {
            let hub = hub.clone();
            let post = Arc::clone(&post);
            let overlay = media_badge(m);
            let corners = grid.corners(tile, inner);
            let gif = match m {
                Media::Gif { stream: Some(url), .. } => {
                    let (w, h) = (tile.w, tile.h);
                    let draw = Rc::new(move |src| rounded_img(src, w, h, corners));
                    let key = ElementId::NamedInteger("gif".into(), gif_key(&post, index));
                    Some(window.with_element_namespace(rcx.scope.clone(), |window| {
                        inline_gif(key, url.as_str(), draw, window, cx)
                    }))
                }
                _ => None,
            };
            div()
                .id(("tile", index))
                .absolute()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let to = Navigate::Media { post: Arc::clone(&post), index };
                    hub.update(cx, |hub, cx| hub.navigate(to, cx));
                })
                .left(px(tile.x))
                .top(px(tile.y))
                .w(px(tile.w))
                .h(px(tile.h))
                .child(rounded_img(
                    remote_image::source(m.preview().url.as_str(), tile.w, tile.h),
                    tile.w,
                    tile.h,
                    corners,
                ))
                .children(gif)
                .children(overlay)
        }))
}

/// Tells a post's GIF tiles apart in a list (without reading the opaque id).
fn gif_key(post: &Post, index: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    (&post.id, index).hash(&mut hasher);
    hasher.finish()
}

/// Outer radius of media boxes and cards.
const RADIUS: f32 = 16.0;
/// Border width of media boxes and cards.
const BORDER: f32 = 1.0;

/// A cover-fit `w` × `h` image that rounds its own outer corners.
///
/// GPUI's `overflow_hidden` clips to a rectangle, so a rounded parent does not
/// round its children: each image rounds the corners that touch the parent's
/// corners, with the parent's radius minus its border.
///
/// The aspect ratio is explicit: without one, a loaded `img` takes the
/// bitmap's, and layout lets it beat the given height. A bitmap taller than
/// its box then overflowed the bottom, and the clip hid its rounded corners.
fn rounded_img(
    source: impl Into<ImageSource>,
    w: f32,
    h: f32,
    corners: media_layout::Corners,
) -> Img {
    let r = px(RADIUS - BORDER);
    img(source)
        .w(px(w))
        .h(px(h))
        .when(h > 0.0, |i| i.aspect_ratio(w / h))
        .object_fit(ObjectFit::Cover)
        .when(corners.top_left, |i| i.rounded_tl(r))
        .when(corners.top_right, |i| i.rounded_tr(r))
        .when(corners.bottom_left, |i| i.rounded_bl(r))
        .when(corners.bottom_right, |i| i.rounded_br(r))
}

pub(crate) fn fmt_duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// The badge drawn over a media's picture: ▶ and the length for a video,
/// "GIF" for a GIF, none for an image.
pub(crate) fn media_badge(m: &Media) -> Option<impl IntoElement + use<>> {
    match m {
        Media::Video { duration, .. } => Some(video_badge("▶", duration.map(fmt_duration))),
        Media::Gif { .. } => Some(video_badge("GIF", None)),
        Media::Image { .. } => None,
    }
}

pub(crate) fn video_badge(label: &'static str, extra: Option<String>) -> impl IntoElement {
    div()
        .absolute()
        .left_2()
        .bottom_2()
        .px_1p5()
        .rounded(px(4.))
        .bg(gpui_kit::black().opacity(0.75))
        .text_color(gpui_kit::white())
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .debug_selector(move || format!("media-badge-{label}"))
        .child(match extra {
            Some(e) => format!("{label} {e}"),
            None => label.to_owned(),
        })
}

fn render_embed(
    id: (&'static str, usize),
    embed: &Embed,
    width: f32,
    hub: &Entity<InteractionsHub>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    match embed {
        Embed::Quote { quoted } => match quoted.as_ref() {
            PostOrGone::Post(q) => quote_card(id, q, width, hub, cx).into_any_element(),
            PostOrGone::Unavailable { reason } => div()
                .mt_2()
                .px_3()
                .py_3()
                .rounded(px(16.))
                .border_1()
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .child(unavailable_text(reason))
                .into_any_element(),
        },
        Embed::Link(card) => link_card(id, card, width, &theme).into_any_element(),
        Embed::Poll(poll) => poll_view(poll, &theme).into_any_element(),
    }
}

fn quote_card(
    id: (&'static str, usize),
    q: &Post,
    width: f32,
    hub: &Entity<InteractionsHub>,
    cx: &mut App,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let card_hub = hub.clone();
    let quoted = q.id.clone();
    v_flex()
        .id(id)
        .mt_2()
        .w(px(width))
        .rounded(px(16.))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            card_hub.update(cx, |hub, cx| hub.navigate(Navigate::Thread(quoted.clone()), cx));
        })
        .child(
            v_flex()
                .px_3()
                .pt_3()
                .pb_2()
                .gap_1()
                .child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .id(("quote-avatar", id.1))
                                .on_click(open_profile(&q.author, hub))
                                .child(avatar(&q.author, 20.0)),
                        )
                        .child(
                            div()
                                .id(("quote-author", id.1))
                                .debug_selector(move || format!("quote-author-{}", id.1))
                                .font_weight(FontWeight::BOLD)
                                .truncate()
                                .hover(|s| s.underline())
                                .on_click(open_profile(&q.author, hub))
                                .child(q.author.display_name.clone()),
                        )
                        .child(div().text_color(theme.muted_foreground).truncate().child(format!(
                            "@{} · {}",
                            q.author.handle,
                            relative_time(q.created_at, Utc::now())
                        ))),
                )
                .child(div().text_color(theme.foreground).child(q.body.text().to_owned())),
        )
        .children(q.media.first().map(|m| {
            let inner = (width - 2.0 * BORDER).max(0.0);
            let h = media_layout::layout(1, m.preview().aspect(), inner).height.min(220.);
            let hub = hub.clone();
            let post = Arc::new(q.clone());
            div()
                .id(("quote-media", id.1))
                .debug_selector(move || format!("quote-media-{}", id.1))
                .relative()
                .w_full()
                .h(px(h))
                .on_click(move |_, _, cx| {
                    // The media opens fullscreen, not the quoted post.
                    cx.stop_propagation();
                    let to = Navigate::Media { post: Arc::clone(&post), index: 0 };
                    hub.update(cx, |hub, cx| hub.navigate(to, cx));
                })
                .child(rounded_img(
                    remote_image::source(m.preview().url.as_str(), inner, h),
                    inner,
                    h,
                    media_layout::Corners::BOTTOM,
                ))
                .children(media_badge(m))
        }))
}

fn link_card(
    id: (&'static str, usize),
    card: &LinkCard,
    width: f32,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let url = card.url.to_string();
    v_flex()
        .id(id)
        .mt_2()
        .w(px(width))
        .rounded(px(16.))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&url);
        })
        .children(card.image.as_ref().map(|image| {
            let inner = (width - 2.0 * BORDER).max(0.0);
            let h = width / 1.91;
            div().w_full().h(px(h)).child(rounded_img(
                remote_image::source(image.url.as_str(), inner, h),
                inner,
                h,
                media_layout::Corners::TOP,
            ))
        }))
        .child(
            v_flex()
                .px_3()
                .py_2()
                .gap_0p5()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div().text_sm().text_color(theme.muted_foreground).child(card.domain.clone()),
                )
                .child(div().text_color(theme.foreground).truncate().child(card.title.clone())),
        )
}

fn poll_view(poll: &Poll, theme: &gpui_kit::component::Theme) -> impl IntoElement {
    let total = poll.total_votes();
    let leader = poll.options.iter().map(|o| o.votes).max().unwrap_or(0);
    v_flex()
        .mt_2()
        .gap_1()
        .children(poll.options.iter().map(|o| {
            let pct = if total == 0 { 0.0 } else { o.votes as f32 / total as f32 };
            let lead = o.votes == leader && total > 0;
            div()
                .relative()
                .h(px(32.))
                .w_full()
                .rounded(px(6.))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(gpui_kit::relative(pct))
                        .bg(if lead { theme.primary.opacity(0.35) } else { theme.muted }),
                )
                .child(
                    h_flex()
                        .absolute()
                        .size_full()
                        .px_3()
                        .justify_between()
                        .when(lead, |d| d.font_weight(FontWeight::BOLD))
                        .child(o.label.clone())
                        .child(format!("{:.0}%", pct * 100.0)),
                )
        }))
        .child(div().text_sm().text_color(theme.muted_foreground).child(format!(
            "{} votes · {}",
            compact_count(total).if_empty("0"),
            if poll.closed { "Final results" } else { "Open" }
        )))
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() { fallback.to_owned() } else { self }
    }
}

/// What an action-bar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    Toggle(Toggle),
    Reply,
    Nothing,
}

/// An action's icon, popping (growing and settling back) right after the
/// viewer turned it on. The fixed-size slot keeps the bar's layout still.
fn action_icon(path: SharedString, popping: bool) -> impl IntoElement {
    const SIZE: f32 = 17.;
    let icon = Icon::empty().path(path).size(px(SIZE));
    div().flex_none().size(px(SIZE)).flex().items_center().justify_center().child(if popping {
        icon.with_animation("pop", Animation::new(motion::POP), |icon, t| {
            icon.size(px(SIZE * motion::pop_scale(t)))
        })
        .into_any_element()
    } else {
        icon.into_any_element()
    })
}

/// The icon an action shows: like and bookmark fill in when on. Lucide's
/// repost arrows are bare strokes with nothing to fill, so only its colour
/// changes.
fn action_icon_path(icon: IconName, on: Click, active: bool) -> SharedString {
    match on {
        Click::Toggle(Toggle::Like) if active => HEART_FILL.into(),
        Click::Toggle(Toggle::Bookmark) if active => BOOKMARK_FILL.into(),
        _ => icon.path(),
    }
}

/// Like and repost hover in the colour they turn when on; the rest in blue.
fn hover_color(on: Click, theme: &gpui_kit::component::Theme) -> Hsla {
    match on {
        Click::Toggle(Toggle::Like) => theme.red,
        Click::Toggle(Toggle::Repost) => theme.green,
        _ => theme.primary,
    }
}

/// Singular and plural noun for a counter's tooltip.
type Noun = (&'static str, &'static str);

fn action_bar(
    key: usize,
    post: &Post,
    eff: crate::state::interactions::Effective,
    pops: &[Toggle],
    rcx: &RowCtx,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    let action = |name: &'static str,
                  icon: IconName,
                  count: Option<(u64, Noun)>,
                  active: Option<Hsla>,
                  on: Click| {
        let color = active.unwrap_or(muted);
        let hub = rcx.hub.clone();
        let post = Arc::new(post.clone());
        h_flex()
            .id((name, key))
            .debug_selector(move || format!("action-{name}-{key}"))
            .gap_1p5()
            // Five actions must fit the narrowest column (310px).
            .min_w(px(36.))
            .text_sm()
            .text_color(color)
            .when(on != Click::Nothing, |d| {
                d.cursor_pointer().hover(|s| s.text_color(hover_color(on, theme)))
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                match on {
                    Click::Toggle(t) => hub.update(cx, |hub, cx| hub.toggle(&post, t, cx)),
                    Click::Reply => {
                        let to = Navigate::Reply(Arc::clone(&post));
                        hub.update(cx, |hub, cx| hub.navigate(to, cx));
                    }
                    Click::Nothing => {}
                }
            })
            .child(action_icon(
                action_icon_path(icon, on, active.is_some()),
                matches!(on, Click::Toggle(t) if pops.contains(&t)),
            ))
            .when_some(count, |d, (n, (one, many))| {
                let full: SharedString = exact_count(n, one, many).into();
                d.tooltip(move |window, cx| {
                    let full = full.clone();
                    Tooltip::element(move |_, _| {
                        div().debug_selector(|| "count-tooltip".into()).child(full.clone())
                    })
                    .build(window, cx)
                })
            })
            .child(count.map(|(n, _)| compact_count(n)).unwrap_or_default())
    };
    let m = eff.metrics;
    h_flex()
        .mt_2()
        .gap_2()
        .justify_between()
        .child(action(
            "reply",
            IconName::MessageCircle,
            Some((m.replies, ("Reply", "Replies"))),
            None,
            Click::Reply,
        ))
        .child(action(
            "repost",
            IconName::Repeat2,
            Some((m.reposts + m.quotes, ("Repost", "Reposts"))),
            eff.viewer.reposted.then_some(theme.green),
            Click::Toggle(Toggle::Repost),
        ))
        .child(action(
            "like",
            IconName::Heart,
            Some((m.likes, ("Like", "Likes"))),
            eff.viewer.liked.then_some(theme.red),
            Click::Toggle(Toggle::Like),
        ))
        .child(action(
            "views",
            IconName::ChartNoAxesColumn,
            m.views.map(|n| (n, ("View", "Views"))),
            None,
            Click::Nothing,
        ))
        .child(action(
            "bookmark",
            IconName::Bookmark,
            None,
            eff.viewer.bookmarked.then_some(theme.blue),
            Click::Toggle(Toggle::Bookmark),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::UserRef;

    #[test]
    fn content_width_never_negative() {
        assert_eq!(content_width(10.0), 0.0);
        assert_eq!(content_width(350.0), 350.0 - 84.0);
    }

    #[test]
    fn span_urls() {
        let mention = SpanKind::Mention(UserRef { id: None, handle: "ferris".into() });
        assert_eq!(span_url(&mention), "https://x.com/ferris");
        assert_eq!(
            span_url(&SpanKind::Hashtag { tag: "gpui".into() }),
            "https://x.com/hashtag/gpui"
        );
        assert_eq!(
            span_url(&SpanKind::Cashtag { symbol: "TSLA".into() }),
            "https://x.com/search?q=%24TSLA"
        );
    }

    fn link(range: std::ops::Range<usize>) -> Span {
        Span { range, kind: SpanKind::Hashtag { tag: "t".into() } }
    }

    #[test]
    fn span_at_finds_the_covering_span() {
        let spans = [link(0..3), link(5..8)];
        let hits: Vec<_> = (0..10).map(|ix| span_at(&spans, ix)).collect();
        let (a, b) = (Some(0), Some(1));
        assert_eq!(hits, [a, a, a, None, None, b, b, b, None, None]);
        assert_eq!(span_at(&[], 0), None);
    }

    proptest::proptest! {
        #[test]
        fn span_at_never_panics_and_is_consistent(
            cuts in proptest::collection::btree_set(0usize..200, 0..20),
            ix in 0usize..250,
        ) {
            // Consecutive pairs of sorted cut points make sorted, disjoint spans.
            let cuts: Vec<usize> = cuts.into_iter().collect();
            let spans: Vec<Span> = cuts.as_chunks::<2>().0.iter().map(|[a, b]| link(*a..*b)).collect();
            let expected = spans.iter().position(|s| s.range.contains(&ix));
            proptest::prop_assert_eq!(span_at(&spans, ix), expected);
        }
    }

    #[test]
    fn duration_format() {
        assert_eq!(fmt_duration(std::time::Duration::from_secs(42)), "0:42");
        assert_eq!(fmt_duration(std::time::Duration::from_secs(3_725)), "62:05");
    }

    #[test]
    fn unavailable_texts_are_human() {
        assert!(unavailable_text(&UnavailableReason::Deleted).contains("deleted"));
        assert_eq!(unavailable_text(&UnavailableReason::Other(Some("x".into()))), "x");
    }

    /// Renders one post in the narrowest column width.
    struct OnePost {
        item: FeedItem,
        hub: Entity<InteractionsHub>,
    }

    impl gpui_kit::Render for OnePost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            let rcx =
                RowCtx { ix: 0, column_width: 310., hub: self.hub.clone(), scope: "test".into() };
            div().w(px(310.)).child(render_item(&self.item, &rcx, window, cx))
        }
    }

    /// The first post of the mock's bookmarks.
    fn mock_post() -> (Post, Arc<dyn twitty_model::Backend>) {
        use twitty_model::{Backend, FeedSource, PageRequest, mock::MockBackend};
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(backend.feed(&FeedSource::Bookmarks, PageRequest::First))
            .unwrap();
        let post = page.items.iter().find_map(|i| i.posts().first().map(|p| (*p).clone())).unwrap();
        (post, Arc::new(backend))
    }

    #[gpui_kit::test]
    fn a_long_name_never_hides_the_time(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (mut post, backend) = mock_post();
        let author = Arc::make_mut(&mut post.author);
        author.display_name = "A display name far too long to fit in any column at all".into();
        author.handle = "a_rather_long_handle_too".into();
        post.reply_to = None;
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();

        let time = cx.debug_bounds("post-time-0").expect("the time is rendered");
        assert!(time.size.width > px(10.), "the time keeps its width: {time:?}");
        assert!(time.right() <= px(310.), "the time is inside the row: {time:?}");
    }

    #[gpui_kit::test]
    fn the_action_bar_has_no_share_and_spaces_its_actions(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, backend) = mock_post();
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();

        assert!(cx.debug_bounds("action-share-0").is_none(), "no share button");
        let names = [
            "action-reply-0",
            "action-repost-0",
            "action-like-0",
            "action-views-0",
            "action-bookmark-0",
        ];
        let bounds: Vec<_> = names
            .into_iter()
            .map(|n| cx.debug_bounds(n))
            .collect::<Option<_>>()
            .expect("every action is rendered");
        for pair in bounds.windows(2) {
            let gap = pair[1].left() - pair[0].right();
            assert!(gap >= px(8.), "actions are at least 8px apart: {pair:?}");
        }
        let last = bounds.last().map(|b| b.right()).unwrap_or_default();
        assert!(last <= px(310. - PAD_X), "the bar fits the narrowest column: {last:?}");
    }

    #[gpui_kit::test]
    fn hovering_a_count_shows_its_full_value(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, backend) = mock_post();
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();
        assert!(cx.debug_bounds("count-tooltip").is_none(), "no tooltip before hovering");

        let like = cx.debug_bounds("action-like-0").expect("the like action is rendered");
        cx.simulate_mouse_move(like.center(), None, Default::default());
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("count-tooltip").is_some(), "the full count shows on hover");
    }

    /// A mock post whose only media is a GIF, rendered with `FakeVideo` and
    /// the given autoplay setting. Counts the hub's navigation requests.
    fn gif_post(
        autoplay: crate::state::playback::Autoplay,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<OnePost>,
        &mut gpui_kit::VisualTestContext,
        Arc<crate::video::FakeVideo>,
        std::rc::Rc<std::cell::Cell<usize>>,
    ) {
        let video = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::ui::gif::GifAutoplay(autoplay));
            crate::video::FakeVideo::install(vec![std::time::Duration::ZERO], cx)
        });
        let (mut post, backend) = mock_post();
        let poster = twitty_model::ImageRef {
            url: "https://pbs.twimg.com/tweet_video_thumb/a.jpg".parse().unwrap(),
            size: Some((480, 270)),
        };
        let stream = Some("https://video.twimg.com/tweet_video/a.mp4".parse().unwrap());
        post.media = vec![Media::Gif { poster, stream, alt: None }];
        post.embed = None;
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let navigations = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = std::rc::Rc::clone(&navigations);
        cx.update(|cx| {
            cx.subscribe(&hub, move |_, _: &Navigate, _| count.set(count.get() + 1)).detach();
        });
        let (view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        redraw(cx);
        (view, cx, video, navigations)
    }

    fn redraw(cx: &mut gpui_kit::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn click(name: &'static str, cx: &mut gpui_kit::VisualTestContext) {
        let at = cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn")).center();
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        redraw(cx);
    }

    #[gpui_kit::test]
    fn a_gif_waits_for_its_button_and_the_button_never_opens_the_viewer(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::state::playback::Autoplay;
        let (_view, cx, video, navigations) = gif_post(Autoplay::Off, cx);
        assert!(video.opened().is_empty(), "off by default: nothing downloads");
        click("gif-play", cx);
        assert_eq!(video.opened(), vec!["https://video.twimg.com/tweet_video/a.mp4"]);
        assert!(cx.debug_bounds("gif-pause").is_some(), "now it offers to pause");
        click("gif-pause", cx);
        assert!(cx.debug_bounds("gif-play").is_some());
        click("gif-play", cx);
        assert_eq!(video.opened().len(), 1, "resumed, not restarted");
        assert_eq!(navigations.get(), 0, "the button never opened the viewer");
    }

    #[gpui_kit::test]
    fn with_autoplay_a_gif_on_screen_plays_on_its_own(cx: &mut gpui_kit::TestAppContext) {
        use crate::state::playback::Autoplay;
        let (_view, cx, video, _) = gif_post(Autoplay::On, cx);
        assert_eq!(video.opened().len(), 1);
        assert!(cx.debug_bounds("gif-pause").is_some());
        click("gif-pause", cx);
        assert!(cx.debug_bounds("gif-play").is_some(), "the viewer's pause beats autoplay");
    }

    #[gpui_kit::test]
    fn the_rest_of_a_gif_tile_still_opens_the_viewer(cx: &mut gpui_kit::TestAppContext) {
        use crate::state::playback::Autoplay;
        let (_view, cx, _, navigations) = gif_post(Autoplay::On, cx);
        let button = cx.debug_bounds("gif-pause").expect("the button is drawn");
        // Well inside the tile, left of the centred button.
        let beside = gpui_kit::point(button.left() - px(60.), button.center().y);
        cx.simulate_click(beside, gpui_kit::Modifiers::default());
        redraw(cx);
        assert_eq!(navigations.get(), 1);
    }

    #[gpui_kit::test]
    fn a_gif_no_longer_drawn_stops_playing(cx: &mut gpui_kit::TestAppContext) {
        use crate::state::playback::Autoplay;
        let (view, cx, video, _) = gif_post(Autoplay::On, cx);
        assert_eq!(video.listening(), 1);
        // As when its row scrolls out of the list.
        view.update(cx, |v, cx| {
            if let FeedItem::Post(post) = &mut v.item {
                post.media.clear();
            }
            cx.notify();
        });
        redraw(cx);
        redraw(cx);
        assert_eq!(video.listening(), 0, "its player and decoder are gone");
    }

    #[gpui_kit::test]
    fn a_quoted_video_or_gif_shows_its_badge(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (outer, backend) = mock_post();
        let poster = twitty_model::ImageRef {
            url: "https://pbs.twimg.com/media/q.jpg".parse().unwrap(),
            size: Some((1280, 720)),
        };
        let quote = |media: Media| {
            let mut quoted = outer.clone();
            quoted.media = vec![media];
            let mut post = outer.clone();
            post.media.clear();
            post.embed = Some(Embed::Quote { quoted: Box::new(PostOrGone::Post(quoted)) });
            FeedItem::Post(post)
        };
        let video = Media::Video {
            poster: poster.clone(),
            variants: Vec::new(),
            duration: Some(std::time::Duration::from_secs(83)),
            alt: None,
        };
        let gif = Media::Gif { poster, stream: None, alt: None };
        for (media, badge) in [(video, "media-badge-▶"), (gif, "media-badge-GIF")] {
            let hub = cx.new(|_| InteractionsHub::new(Arc::clone(&backend)));
            let item = quote(media);
            let (_view, cx) = cx.add_window_view(|_, _| OnePost { item, hub });
            cx.run_until_parked();
            let card = cx.debug_bounds("quote-media-0").expect("the quoted media");
            let shown = cx.debug_bounds(badge).unwrap_or_else(|| panic!("{badge} is drawn"));
            assert!(card.contains(&shown.center()), "over the quoted media");
        }
    }

    /// A box like a media tile, holding one rounded image.
    struct Tile {
        image: Arc<gpui_kit::Image>,
    }

    impl gpui_kit::Render for Tile {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            div().relative().w(px(200.)).h(px(100.)).child(
                div().absolute().left_0().top_0().w(px(200.)).h(px(100.)).child(
                    rounded_img(Arc::clone(&self.image), 200., 100., media_layout::Corners::ALL)
                        .debug_selector(|| "tile-image".into()),
                ),
            )
        }
    }

    #[gpui_kit::test]
    fn a_loaded_image_fills_its_tile_whatever_its_own_aspect(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        // 40×120: far taller than the 2:1 tile, like a portrait clamped to 3:4.
        let png = include_bytes!("../../tests/fixtures/tall-40x120.png").to_vec();
        let image = Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png));
        let (_view, cx) = cx.add_window_view(|_, _| Tile { image });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();

        let bounds = cx.debug_bounds("tile-image").expect("the image is rendered");
        // Taller than the tile, its rounded bottom corners would be clipped off.
        assert_eq!(bounds.size, gpui_kit::size(px(200.), px(100.)));
    }

    #[gpui_kit::test]
    fn the_handle_is_smaller_than_the_name(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (mut post, backend) = mock_post();
        let author = Arc::make_mut(&mut post.author);
        author.display_name = "Al".into();
        author.handle = "al".into();
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();

        let name = cx.debug_bounds("post-name-0").expect("the name is rendered");
        let handle = cx.debug_bounds("post-handle-0").expect("the handle is rendered");
        let time = cx.debug_bounds("post-time-0").expect("the time is rendered");
        assert!(handle.size.height < name.size.height, "smaller text: {handle:?} vs {name:?}");
        assert_eq!(handle.size.height, time.size.height, "same size as the time");
        assert!(handle.bottom() <= name.bottom(), "inside the name's line: {handle:?}");

        // Each box's baseline sits `baseline_from_bottom` above its bottom.
        let mut baseline = |bounds: gpui_kit::Bounds<gpui_kit::Pixels>, weight, scale: f32| {
            cx.update(|window, cx| {
                let size = window.rem_size() * scale;
                let font = gpui_kit::Font {
                    family: cx.theme().font_family.clone(),
                    weight,
                    ..window.text_style().font()
                };
                let ts = window.text_system();
                let id = ts.resolve_font(&font);
                let from_bottom = baseline_from_bottom(
                    bounds.size.height,
                    ts.ascent(id, size),
                    ts.descent(id, size),
                );
                bounds.bottom() - from_bottom
            })
        };
        let name_line = baseline(name, FontWeight::BOLD, 1.);
        let handle_line = baseline(handle, FontWeight::NORMAL, HANDLE_SCALE);
        assert!(
            (name_line - handle_line - px(HANDLE_RAISE)).abs() < px(0.1),
            "the handle's baseline {handle_line:?} sits {HANDLE_RAISE}px above the name's {name_line:?}"
        );
    }

    #[test]
    fn the_baseline_sits_above_the_descent() {
        // 20px line, 12px ascent, 4px descent: 2px padding, then the descent.
        assert_eq!(baseline_from_bottom(px(20.), px(12.), px(4.)), px(6.));
    }

    #[gpui_kit::test]
    fn the_time_sits_at_the_far_right(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (mut post, backend) = mock_post();
        let author = Arc::make_mut(&mut post.author);
        author.display_name = "Al".into();
        author.handle = "al".into();
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();

        let time = cx.debug_bounds("post-time-0").expect("the time is rendered");
        let edge = px(310. - PAD_X);
        assert!((time.right() - edge).abs() < px(1.), "the time ends at {edge:?}: {time:?}");
    }

    #[gpui_kit::test]
    fn clicking_the_time_opens_vxtwitter_not_the_thread(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, _) = mock_post();
        let link = links::vx_link(&post);
        assert_eq!(navigations(cx, FeedItem::Post(post), "post-time-0"), vec![]);
        assert_eq!(cx.opened_url(), Some(link));
    }

    #[gpui_kit::test]
    fn right_clicking_a_post_copies_its_vxtwitter_link(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, backend) = mock_post();
        let link = links::vx_link(&post);
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item: FeedItem::Post(post), hub });
        cx.run_until_parked();

        let press = cx.debug_bounds("post-time-0").expect("the time is rendered").center();
        let press = gpui_kit::point(px(60.), press.y);
        cx.simulate_mouse_down(press, gpui_kit::MouseButton::Right, Default::default());
        cx.simulate_mouse_up(press, gpui_kit::MouseButton::Right, Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied, Some(link));
    }

    /// Renders `item`, clicks the element `target` and returns what the
    /// row asked the app to open.
    fn navigations(
        cx: &mut gpui_kit::TestAppContext,
        item: FeedItem,
        target: &'static str,
    ) -> Vec<Navigate> {
        navigations_at(cx, item, target, |b| b.center())
    }

    /// The navigations requested by a click on `target` at `at(its bounds)`.
    fn navigations_at(
        cx: &mut gpui_kit::TestAppContext,
        item: FeedItem,
        target: &'static str,
        at: impl Fn(gpui_kit::Bounds<gpui_kit::Pixels>) -> gpui_kit::Point<gpui_kit::Pixels>,
    ) -> Vec<Navigate> {
        use std::{cell::RefCell, rc::Rc};
        let backend: Arc<dyn twitty_model::Backend> =
            Arc::new(twitty_model::mock::MockBackend::new());
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        cx.update(|cx| {
            cx.subscribe(&hub, move |_, e: &Navigate, _| sink.borrow_mut().push(e.clone())).detach()
        });
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item, hub });
        cx.run_until_parked();
        let bounds = cx.debug_bounds(target).unwrap_or_else(|| panic!("{target} rendered"));
        cx.simulate_click(at(bounds), gpui_kit::Modifiers::default());
        cx.run_until_parked();
        seen.take()
    }

    fn mock_items(source: twitty_model::FeedSource) -> Vec<FeedItem> {
        use twitty_model::{Backend, PageRequest, mock::MockBackend};
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(MockBackend::new().feed(&source, PageRequest::First))
            .unwrap()
            .items
    }

    #[gpui_kit::test]
    fn clicking_a_mention_opens_the_profile_in_the_app(cx: &mut gpui_kit::TestAppContext) {
        use twitty_model::FeedSource;
        cx.update(gpui_kit::init);
        // The mock's rich posts start with a mention.
        let post = mock_items(FeedSource::Bookmarks)
            .iter()
            .flat_map(|i| i.posts())
            .find(|p| p.body.spans().first().is_some_and(|s| s.range.start == 0))
            .cloned()
            .expect("the mock has a post starting with a mention");
        let SpanKind::Mention(user) = &post.body.spans()[0].kind else {
            panic!("the post starts with a mention")
        };
        let user = user.clone();
        let start =
            |b: gpui_kit::Bounds<gpui_kit::Pixels>| b.origin + gpui_kit::point(px(6.), px(8.));
        assert_eq!(
            navigations_at(cx, FeedItem::Post(post), "body-text-0", start),
            vec![Navigate::Mention(user)],
            "the mention opens in the app, not the browser or the thread"
        );
    }

    #[gpui_kit::test]
    fn clicking_the_avatar_or_name_opens_the_profile(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, _) = mock_post();
        let author = Arc::clone(&post.author);
        for target in ["post-avatar-0", "post-name-0"] {
            assert_eq!(
                navigations(cx, FeedItem::Post(post.clone()), target),
                vec![Navigate::Profile(Arc::clone(&author))],
                "{target} opens the profile, not the thread"
            );
        }
    }

    #[gpui_kit::test]
    fn reposters_quoted_authors_and_notification_actors_open_their_profile(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use twitty_model::{FeedSource, NotificationFilter};
        cx.update(gpui_kit::init);
        let items = mock_items(FeedSource::Bookmarks);

        let repost = items.iter().find(|i| matches!(i, FeedItem::Repost { .. })).unwrap();
        let FeedItem::Repost { by, .. } = repost else { unreachable!() };
        assert_eq!(
            navigations(cx, repost.clone(), "post-reposter-0"),
            vec![Navigate::Profile(Arc::clone(by))]
        );

        let (quoting, quoted) = items
            .iter()
            .find_map(|i| match i {
                FeedItem::Post(p) => match p.embed.as_ref() {
                    Some(Embed::Quote { quoted }) => match quoted.as_ref() {
                        PostOrGone::Post(q) => Some((p.clone(), Arc::clone(&q.author))),
                        PostOrGone::Unavailable { .. } => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .expect("the mock has a quote");
        assert_eq!(
            navigations(cx, FeedItem::Post(quoting), "quote-author-0"),
            vec![Navigate::Profile(quoted)],
            "not the quoted post"
        );

        let notification =
            mock_items(FeedSource::Notifications { filter: NotificationFilter::All })
                .into_iter()
                .find(|i| matches!(i, FeedItem::Notification(_)))
                .unwrap();
        let FeedItem::Notification(n) = &notification else { unreachable!() };
        let actor = Arc::clone(&n.actors[0]);
        assert_eq!(navigations(cx, notification, "notif-actor-0"), vec![Navigate::Profile(actor)]);
    }

    #[gpui_kit::test]
    fn clicking_a_quoted_posts_media_opens_it_fullscreen(cx: &mut gpui_kit::TestAppContext) {
        use twitty_model::FeedSource;
        cx.update(gpui_kit::init);
        let items = mock_items(FeedSource::Bookmarks);
        let image = items
            .iter()
            .flat_map(|i| i.posts())
            .flat_map(|p| p.media.iter())
            .find(|m| matches!(m, Media::Image { .. }))
            .cloned()
            .expect("the mock has an image");
        let (mut quoting, mut quoted) = items
            .iter()
            .find_map(|i| match i {
                FeedItem::Post(p) => match p.embed.as_ref() {
                    Some(Embed::Quote { quoted }) => match quoted.as_ref() {
                        PostOrGone::Post(q) => Some((p.clone(), q.clone())),
                        PostOrGone::Unavailable { .. } => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .expect("the mock has a quote");
        quoted.media = vec![image];
        quoting.embed = Some(Embed::Quote { quoted: Box::new(PostOrGone::Post(quoted.clone())) });

        assert_eq!(
            navigations(cx, FeedItem::Post(quoting), "quote-media-0"),
            vec![Navigate::Media { post: Arc::new(quoted), index: 0 }],
            "not the quoted post"
        );
    }

    #[gpui_kit::test]
    fn like_and_repost_hover_in_their_own_colour(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let theme = cx.theme();
            assert_eq!(hover_color(Click::Toggle(Toggle::Like), theme), theme.red);
            assert_eq!(hover_color(Click::Toggle(Toggle::Repost), theme), theme.green);
            assert_eq!(hover_color(Click::Toggle(Toggle::Bookmark), theme), theme.primary);
            assert_eq!(hover_color(Click::Reply, theme), theme.primary);
        });
    }

    #[test]
    fn liked_and_bookmarked_icons_fill_in() {
        let like = Click::Toggle(Toggle::Like);
        let bookmark = Click::Toggle(Toggle::Bookmark);
        let repost = Click::Toggle(Toggle::Repost);
        assert_eq!(action_icon_path(IconName::Heart, like, true).as_ref(), HEART_FILL);
        assert_eq!(action_icon_path(IconName::Heart, like, false), IconName::Heart.path());
        assert_eq!(action_icon_path(IconName::Bookmark, bookmark, true).as_ref(), BOOKMARK_FILL);
        assert_eq!(
            action_icon_path(IconName::Bookmark, bookmark, false),
            IconName::Bookmark.path()
        );
        assert_eq!(action_icon_path(IconName::Repeat2, repost, true), IconName::Repeat2.path());
    }

    #[test]
    fn chain_positions() {
        assert_eq!(Chain::at(0, 1), Chain::Alone);
        assert_eq!([0, 1].map(|k| Chain::at(k, 2)), [Chain::First, Chain::Last]);
        assert_eq!([0, 1, 2].map(|k| Chain::at(k, 3)), [Chain::First, Chain::Middle, Chain::Last]);
        assert!(!Chain::Alone.joined_above() && !Chain::Alone.joined_below());
        assert!(Chain::Middle.joined_above() && Chain::Middle.joined_below());
    }

    #[gpui_kit::test]
    fn a_threads_line_runs_unbroken_from_post_to_post(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (post, backend) = mock_post();
        let mut reply = post.clone();
        reply.id = twitty_model::PostId::new("reply");
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let item = FeedItem::Thread(vec![post, reply]);
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item, hub });
        cx.run_until_parked();

        let drawn = [
            "post-row-0",
            "post-row-1",
            "line-below-0",
            "line-above-1",
            "post-avatar-0",
            "post-avatar-1",
            "action-like-0",
            "action-like-1",
        ]
        .map(|name| cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn")));
        let [first, second, below, above, avatar_0, avatar_1, like_0, like_1] = drawn;
        let near = |a: Pixels, b: Pixels| (a - b).abs() < px(0.5);
        assert!(near(first.bottom(), second.top()), "the posts touch");
        assert!(near(below.top(), avatar_0.bottom() + px(LINE_GAP)), "starts under the avatar");
        assert!(near(below.bottom(), first.bottom()), "runs to the post's edge: {below:?}");
        assert!(near(above.top(), second.top()), "picks up at the next post's edge");
        assert!(near(above.bottom(), avatar_1.top() - px(LINE_GAP)), "and stops at its avatar");
        assert!(near(below.center().x, above.center().x), "one straight line");
        assert!(cx.debug_bounds("line-above-0").is_none(), "nothing above the first post");
        assert!(cx.debug_bounds("line-below-1").is_none(), "nor below the last");

        // Every post keeps room under its actions, so the hover tint never
        // ends flush with them.
        for (row, like) in [(first, like_0), (second, like_1)] {
            assert!(row.bottom() - like.bottom() >= px(8.), "room under {like:?} in {row:?}");
        }
    }

    #[gpui_kit::test]
    fn a_notification_shows_its_time_at_the_far_right(cx: &mut gpui_kit::TestAppContext) {
        use twitty_model::{FeedSource, NotificationFilter};
        cx.update(gpui_kit::init);
        let item = mock_items(FeedSource::Notifications { filter: NotificationFilter::All })
            .into_iter()
            .find(|i| matches!(i, FeedItem::Notification(_)))
            .expect("the mock has a notification");
        let backend: Arc<dyn twitty_model::Backend> =
            Arc::new(twitty_model::mock::MockBackend::new());
        let hub = cx.new(|_| InteractionsHub::new(backend));
        let (_view, cx) = cx.add_window_view(|_, _| OnePost { item, hub });
        cx.run_until_parked();

        let time = cx.debug_bounds("notif-time-0").expect("the time is rendered");
        let edge = px(310. - PAD_X);
        assert!((time.right() - edge).abs() < px(1.), "the time ends at {edge:?}: {time:?}");
        assert!(time.size.width > px(0.), "and says something");
    }
}
