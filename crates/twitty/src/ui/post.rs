//! Rendering of feed rows: posts, reposts, threads and their embeds.

use std::sync::Arc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, App, Entity, FontWeight, HighlightStyle, Hsla, InteractiveText, IntoElement,
    ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    StyledText, Window, component::avatar::Avatar, component::*, div, img, prelude::*, px,
};
use twitty_model::{
    Embed, FeedItem, LinkCard, Media, NotificationKind, Poll, Post, PostOrGone, Profile, RichText,
    SpanKind, UnavailableReason, VerifiedKind,
};

use crate::{
    format::{compact_count, relative_time},
    state::{interactions::Toggle, media_layout},
    ui::hub::InteractionsHub,
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
                            .child(
                                h_flex()
                                    .gap_1()
                                    .children(n.actors.iter().take(6).map(|a| avatar(a, 32.0))),
                            )
                            .child(rich_text(("notif", rcx.ix), &n.text, theme.link, cx))
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

fn avatar(profile: &Profile, size: f32) -> impl IntoElement {
    let mut a = Avatar::new().name(profile.display_name.clone()).size(px(size));
    if let Some(src) = &profile.avatar {
        a = a.src(src.url.as_str().to_owned());
    }
    a
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
    let eff = rcx.hub.read(cx).effective(post);
    let width = content_width(rcx.column_width);
    let key = rcx.ix * 16 + sub;

    let header = h_flex()
        .gap_1()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .truncate()
                .child(post.author.display_name.clone()),
        )
        .children(post.author.badges.verified.map(|v| verified_badge(v, &theme)))
        .child(div().text_color(theme.muted_foreground).truncate().flex_shrink_0().flex_1().child(
            format!("@{} · {}", post.author.handle, relative_time(post.created_at, Utc::now())),
        ));

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
        content = content.child(rich_text(("body", key), &post.body, theme.link, cx));
    }
    if !post.media.is_empty() {
        content = content.child(media_grid(("media", key), &post.media, width, post, &theme));
    }
    if let Some(embed) = &post.embed {
        content = content.child(render_embed(("embed", key), embed, width, window, cx));
    }
    content = content.child(action_bar(key, post, eff, rcx, &theme));

    let avatar_col = v_flex()
        .items_center()
        .flex_none()
        .w(px(AVATAR))
        .h_full()
        .child(avatar(&post.author, AVATAR))
        .when(connector, |col| {
            col.child(div().mt_1().w(px(2.)).flex_1().min_h(px(12.)).bg(theme.border))
        });

    v_flex()
        .w_full()
        .px(px(PAD_X))
        .pt_3()
        .when(!connector, |d| d.pb_2())
        .hover(|s| s.bg(theme.list_hover))
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
                    .child(format!("{} reposted", by.display_name)),
            )
        })
        .child(h_flex().items_start().gap(px(GAP)).child(avatar_col).child(content))
        .into_any_element()
}

fn verified_badge(kind: VerifiedKind, theme: &gpui_kit::component::Theme) -> impl IntoElement {
    let color = match kind {
        VerifiedKind::Individual => theme.blue,
        VerifiedKind::Business => theme.yellow,
        VerifiedKind::Government => theme.muted_foreground,
    };
    Icon::new(IconName::BadgeCheck).size(px(16.)).text_color(color).flex_none()
}

/// Text with clickable mentions, hashtags and links.
fn rich_text(
    id: (&'static str, usize),
    text: &RichText,
    link: Hsla,
    _cx: &mut App,
) -> impl IntoElement {
    let spans = text.spans().to_vec();
    let highlights = spans
        .iter()
        .map(|s| (s.range.clone(), HighlightStyle { color: Some(link), ..Default::default() }));
    let styled =
        StyledText::new(SharedString::from(text.text().to_owned())).with_highlights(highlights);
    let ranges = spans.iter().map(|s| s.range.clone()).collect();
    div().w_full().child(InteractiveText::new(id, styled).on_click(ranges, move |ix, _, cx| {
        if let Some(span) = spans.get(ix) {
            cx.open_url(&span_url(&span.kind));
        }
    }))
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
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let grid =
        media_layout::layout(media.len(), media.first().and_then(|m| m.preview().aspect()), width);
    let url = post.url.to_string();
    div()
        .id(id)
        .relative()
        .mt_2()
        .w(px(width))
        .h(px(grid.height))
        .rounded(px(16.))
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&url);
        })
        .children(grid.tiles.iter().zip(media).map(|(tile, m)| {
            let overlay = match m {
                Media::Video { duration, .. } => Some(video_badge("▶", duration.map(fmt_duration))),
                Media::Gif { .. } => Some(video_badge("GIF", None)),
                Media::Image { .. } => None,
            };
            div()
                .absolute()
                .left(px(tile.x))
                .top(px(tile.y))
                .w(px(tile.w))
                .h(px(tile.h))
                .child(
                    img(m.preview().url.as_str().to_owned())
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .children(overlay)
        }))
}

fn fmt_duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn video_badge(label: &'static str, extra: Option<String>) -> impl IntoElement {
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
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    match embed {
        Embed::Quote { quoted } => match quoted.as_ref() {
            PostOrGone::Post(q) => quote_card(id, q, width, cx).into_any_element(),
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

fn quote_card(id: (&'static str, usize), q: &Post, width: f32, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme().clone();
    let url = q.url.to_string();
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
            cx.open_url(&url);
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
                        .child(avatar(&q.author, 20.0))
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .truncate()
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
            let grid = media_layout::layout(1, m.preview().aspect(), width);
            div().w_full().h(px(grid.height.min(220.))).child(
                img(m.preview().url.as_str().to_owned()).size_full().object_fit(ObjectFit::Cover),
            )
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
            div()
                .w_full()
                .h(px(width / 1.91))
                .child(img(image.url.as_str().to_owned()).size_full().object_fit(ObjectFit::Cover))
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

fn action_bar(
    key: usize,
    post: &Post,
    eff: crate::state::interactions::Effective,
    rcx: &RowCtx,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    let action = |name: &'static str,
                  icon: IconName,
                  count: Option<u64>,
                  active: Option<Hsla>,
                  toggle: Option<Toggle>| {
        let color = active.unwrap_or(muted);
        let hub = rcx.hub.clone();
        let post = post.clone();
        let url = post.url.to_string();
        h_flex()
            .id((name, key))
            .gap_1()
            .min_w(px(48.))
            .text_sm()
            .text_color(color)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.primary))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                match toggle {
                    Some(t) => hub.update(cx, |hub, cx| hub.toggle(&post, t, cx)),
                    None => cx.open_url(&url),
                }
            })
            .child(Icon::new(icon).size(px(17.)))
            .child(count.map(compact_count).unwrap_or_default())
    };
    let m = eff.metrics;
    h_flex()
        .mt_1()
        .justify_between()
        .child(action("reply", IconName::MessageCircle, Some(m.replies), None, None))
        .child(action(
            "repost",
            IconName::Repeat2,
            Some(m.reposts + m.quotes),
            eff.viewer.reposted.then_some(theme.green),
            Some(Toggle::Repost),
        ))
        .child(action(
            "like",
            IconName::Heart,
            Some(m.likes),
            eff.viewer.liked.then_some(theme.red),
            Some(Toggle::Like),
        ))
        .child(action("views", IconName::ChartNoAxesColumn, m.views, None, None))
        .child(action(
            "bookmark",
            IconName::Bookmark,
            None,
            eff.viewer.bookmarked.then_some(theme.blue),
            Some(Toggle::Bookmark),
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
}
