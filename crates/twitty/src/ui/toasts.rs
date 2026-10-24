//! Draws [`Toasts`]: a stack in the bottom-right corner of the window, green for
//! successes and red for errors. A click on one dismisses it.

use std::{rc::Rc, time::Instant};

use gpui_kit::{
    Animation, AnimationExt as _, App, ElementId, IntoElement, Window, assets::IconName,
    component::*, div, prelude::*, px,
};

use crate::state::{
    motion,
    toasts::{EXIT, ToastKind, Toasts},
};

/// Distance from the window's bottom and right edges.
const MARGIN: f32 = 16.0;
/// Widest a toast gets; longer messages wrap.
const MAX_WIDTH: f32 = 420.0;

/// The toast layer, or `None` when nothing is shown. `dismiss` gets the id of
/// a clicked toast.
pub(crate) fn render(
    toasts: &Toasts,
    now: Instant,
    theme: &Theme,
    reduce_motion: bool,
    dismiss: impl Fn(&u64, &mut Window, &mut App) + 'static,
) -> Option<impl IntoElement> {
    if toasts.items().is_empty() {
        return None;
    }
    let dismiss = Rc::new(dismiss);
    let enter = theme.motion.clone();
    let rows = toasts.items().iter().filter_map(|toast| {
        let leaving = toast.is_leaving(now);
        if leaving && reduce_motion {
            return None;
        }
        let (bg, fg, icon) = match toast.kind {
            ToastKind::Success => (theme.success, theme.success_foreground, IconName::CircleCheck),
            ToastKind::Error => (theme.danger, theme.danger_foreground, IconName::CircleX),
        };
        let id = toast.id;
        let card = h_flex()
            .id(ElementId::NamedInteger("toast".into(), id))
            .debug_selector(move || format!("toast-{id}"))
            .occlude()
            .max_w(px(MAX_WIDTH))
            .items_start()
            .gap_2()
            .px_4()
            .py_2p5()
            .rounded(theme.radius_lg)
            .shadow_lg()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .child(Icon::new(icon).size(px(16.)).mt(px(2.)).flex_none())
            .child(div().min_w_0().child(toast.text.clone()));
        let card = if leaving {
            // Fading out: no longer clickable.
            card.with_animation(
                ElementId::NamedInteger("toast-out".into(), id),
                Animation::new(EXIT),
                |d, t| d.opacity(1. - t),
            )
            .into_any_element()
        } else {
            let dismiss = Rc::clone(&dismiss);
            let card = card.cursor_pointer().on_click(move |_, window, cx| {
                cx.stop_propagation();
                dismiss(&id, window, cx);
            });
            if reduce_motion {
                card.into_any_element()
            } else {
                let easing = enter.easing_enter.clone();
                card.with_animation(
                    ElementId::NamedInteger("toast-in".into(), id),
                    Animation::new(enter.duration_normal)
                        .with_easing(move |t| easing.sample(t).clamp(0., 1.)),
                    |d, t| d.opacity(t).top(px(motion::TRAVEL * (1. - t))),
                )
                .into_any_element()
            }
        };
        Some(div().relative().child(card))
    });
    Some(
        v_flex()
            .debug_selector(|| "toasts".into())
            .absolute()
            .right(px(MARGIN))
            .bottom(px(MARGIN))
            .items_end()
            .gap_2()
            .children(rows),
    )
}
