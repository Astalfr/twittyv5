//! Rendering of feed rows: posts, reposts, threads and their embeds.

use std::sync::Arc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, ClipboardItem, ElementId, Entity,
    FontWeight, HighlightStyle, Hsla, ImageSource, Img, InteractiveText, IntoElement, ObjectFit,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage, StyledText,
    UnderlineStyle, Window,
    component::avatar::Avatar,
    component::button::*,
    component::hover_card::HoverCard,
    component::menu::{ContextMenuExt, PopupMenuItem},
    component::tooltip::Tooltip,
    component::*,
    div, img,
    prelude::*,
    px,
};
use twitty_model::{
    Embed, FeedItem, LinkCard, Media, NotificationKind, Poll, Post, PostOrGone, Profile, RichText,
    Span, SpanKind, UnavailableReason, VerifiedKind,
};

use crate::{
    format::{compact_count, exact_time, relative_time},
    state::{interactions::Toggle, links, media_layout, motion},
    ui::hub::{InteractionsHub, Navigate},
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
}

/// Renders one feed row.
pub fn render_item(item: &FeedItem, rcx: &RowCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let row = div().w_full().border_b_1().border_color(theme.border);
    match item {
        FeedItem::Post(post) => {
            row.child(post_row(post, None, false, rcx, 0, window, cx)).into_any_element()
        }
        FeedItem::Repost { by, post } => {
            row.child(post_row(post, Some(by), false, rcx, 0, window, cx)).into_any_element()
        }
        FeedItem::Thread(posts) => {
            let last = posts.len().saturating_sub(1);
            row.children(
                posts
                    .iter()
                    .enumerate()
                    .map(|(k, p)| post_row(p, None, k < last, rcx, k, window, cx)),
            )
            .into_any_element()
        }
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
                            .child(h_flex().gap_1().children(
                                n.actors.iter().take(6).enumerate().map(|(k, a)| {
                                    let key = rcx.ix * 8 + k;
                                    div()
                                        .id(("notif-actor", key))
                                        .debug_selector(move || format!("notif-actor-{key}"))
                                        .cursor_pointer()
                                        .on_click(open_profile(a, &rcx.hub))
                                        .child(avatar(a, 32.0))
                                }),
                            ))
                            .child(rich_text(("notif", rcx.ix), &n.text, theme.link, window, cx))
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
            .child(Button::new("open-profile").small().label("Open column").on_click(
                move |_, _, cx| {
                    let to = Navigate::UserColumn(Arc::clone(&p));
                    hub.update(cx, |hub, cx| hub.navigate(to, cx));
                },
            ))
    })
}

fn post_row(
    post: &Post,
    reposted_by: Option<&Arc<Profile>>,
    connector: bool,
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
                .min_w_0()
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
        && !connector
        && sub == 0
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
        content = content.child(rich_text(("body", key), &post.body, theme.link, window, cx));
    }
    if !post.media.is_empty() {
        content =
            content.child(media_grid(("media", key), &post.media, width, post, &rcx.hub, &theme));
    }
    if let Some(embed) = &post.embed {
        content = content.child(render_embed(("embed", key), embed, width, &rcx.hub, window, cx));
    }
    content = content.child(action_bar(key, post, eff, &pops, rcx, &theme));

    let avatar_col = v_flex()
        .items_center()
        .flex_none()
        .w(px(AVATAR))
        .h_full()
        .child(profile_hover(key, &post.author, &rcx.hub))
        .when(connector, |col| {
            col.child(div().mt_1().w(px(2.)).flex_1().min_h(px(12.)).bg(theme.border))
        });

    let hub = rcx.hub.clone();
    let id = post.id.clone();
    v_flex()
        .id(("post", key))
        .w_full()
        .px(px(PAD_X))
        .pt_3()
        .when(!connector, |d| d.pb_2())
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
        .child(h_flex().items_start().gap(px(GAP)).child(avatar_col).child(content))
        .context_menu(move |menu, _, _| {
            let link = link.clone();
            menu.item(PopupMenuItem::new("Copy tweet link").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
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
/// underlined.
pub(crate) fn rich_text(
    id: (&'static str, usize),
    text: &RichText,
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
    let text = InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| {
            if let Some(span) = spans.get(ix) {
                // A link click must not also open the post's thread.
                cx.stop_propagation();
                cx.open_url(&span_url(&span.kind));
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

/// Where a click on a span leads (until in-app navigation exists).
pub fn span_url(kind: &SpanKind) -> String {
    match kind {
        SpanKind::Mention(user) => format!("https://x.com/{}", user.handle),
        SpanKind::Hashtag { tag } => format!("https://x.com/hashtag/{tag}"),
        SpanKind::Cashtag { symbol } => format!("https://x.com/search?q=%24{symbol}"),
        SpanKind::Link { url } => url.to_string(),
    }
}

fn media_grid(
    id: (&'static str, usize),
    media: &[Media],
    width: f32,
    post: &Post,
    hub: &Entity<InteractionsHub>,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
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
            let overlay = match m {
                Media::Video { duration, .. } => Some(video_badge("▶", duration.map(fmt_duration))),
                Media::Gif { .. } => Some(video_badge("GIF", None)),
                Media::Image { .. } => None,
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
                    m.preview().url.to_string(),
                    tile.w,
                    tile.h,
                    grid.corners(tile, inner),
                ))
                .children(overlay)
        }))
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
            div().w_full().h(px(h)).child(rounded_img(
                m.preview().url.to_string(),
                inner,
                h,
                media_layout::Corners::BOTTOM,
            ))
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
                image.url.to_string(),
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
    OpenInBrowser,
    Nothing,
}

/// An action's icon, popping (growing and settling back) right after the
/// viewer turned it on. The fixed-size slot keeps the bar's layout still.
fn action_icon(icon: IconName, popping: bool) -> impl IntoElement {
    const SIZE: f32 = 17.;
    let icon = Icon::new(icon).size(px(SIZE));
    div().flex_none().size(px(SIZE)).flex().items_center().justify_center().child(if popping {
        icon.with_animation("pop", Animation::new(motion::POP), |icon, t| {
            icon.size(px(SIZE * motion::pop_scale(t)))
        })
        .into_any_element()
    } else {
        icon.into_any_element()
    })
}

/// Like and repost hover in the colour they turn when on; the rest in blue.
fn hover_color(on: Click, theme: &gpui_kit::component::Theme) -> Hsla {
    match on {
        Click::Toggle(Toggle::Like) => theme.red,
        Click::Toggle(Toggle::Repost) => theme.green,
        _ => theme.primary,
    }
}

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
                  count: Option<u64>,
                  active: Option<Hsla>,
                  on: Click| {
        let color = active.unwrap_or(muted);
        let hub = rcx.hub.clone();
        let post = Arc::new(post.clone());
        h_flex()
            .id((name, key))
            .gap_1()
            // Six actions must fit the narrowest column (310px).
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
                    Click::OpenInBrowser => cx.open_url(post.url.as_str()),
                    Click::Nothing => {}
                }
            })
            .child(action_icon(icon, matches!(on, Click::Toggle(t) if pops.contains(&t))))
            .child(count.map(compact_count).unwrap_or_default())
    };
    let m = eff.metrics;
    h_flex()
        .mt_1()
        .justify_between()
        .child(action("reply", IconName::MessageCircle, Some(m.replies), None, Click::Reply))
        .child(action(
            "repost",
            IconName::Repeat2,
            Some(m.reposts + m.quotes),
            eff.viewer.reposted.then_some(theme.green),
            Click::Toggle(Toggle::Repost),
        ))
        .child(action(
            "like",
            IconName::Heart,
            Some(m.likes),
            eff.viewer.liked.then_some(theme.red),
            Click::Toggle(Toggle::Like),
        ))
        .child(action("views", IconName::ChartNoAxesColumn, m.views, None, Click::Nothing))
        .child(action(
            "bookmark",
            IconName::Bookmark,
            None,
            eff.viewer.bookmarked.then_some(theme.blue),
            Click::Toggle(Toggle::Bookmark),
        ))
        .child(action("share", IconName::Share, None, None, Click::OpenInBrowser))
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
            let rcx = RowCtx { ix: 0, column_width: 310., hub: self.hub.clone() };
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
        cx.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
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
}
