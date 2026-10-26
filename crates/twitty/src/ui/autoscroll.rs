//! Middle-click autoscroll (`state::autoscroll`). A layer over the whole
//! window watches the mouse; while autoscroll is on, each tick turns the
//! pointer's distance from the anchor into a scroll step.
//!
//! Like Chrome, the scroll target is picked at the press and kept until the
//! end. A view that scrolls vertically and can be slid away by the deck (a
//! column) latches itself with [`offer_vertical`] and is scrolled directly,
//! even once it is off screen. Everything else (the deck sideways, sheets,
//! dialogs) gets wheel events at the anchor, so it scrolls the way a wheel
//! would scroll it.
//!
//! The layer has no hitbox, so it never takes hover or clicks from the views
//! under it; it listens with raw window listeners, which also see presses over
//! occluding sheets and dialogs.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_kit::{
    App, Context, DispatchPhase, Global, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PlatformInput, Render, ScrollDelta, ScrollWheelEvent, Styled,
    Task, TouchPhase, Window, canvas, component::ActiveTheme, div, point, prelude::*, px,
};

use crate::state::autoscroll::{Autoscroll, Release, Vec2};

/// Time between two scroll steps.
const TICK: Duration = Duration::from_millis(16);
/// Diameter (px) of the anchor's marker.
const MARKER: f32 = 28.0;
const MARKER_DOT: f32 = 6.0;

/// Scrolls a latched view down by the given px (negative: up).
type ScrollFn = Rc<dyn Fn(f32, &mut App)>;

/// The view that offered itself during the current press.
#[derive(Default)]
struct Offered(Option<ScrollFn>);

impl Global for Offered {}

/// For a view's capture-phase middle-press listener: if the press starts an
/// autoscroll, its vertical steps go to `scroll` (px, positive: down) until it
/// ends, wherever the view is by then. Views under the pointer offer
/// themselves outermost first, so the innermost one wins.
pub(crate) fn offer_vertical(scroll: impl Fn(f32, &mut App) + 'static, cx: &mut App) {
    cx.default_global::<Offered>().0 = Some(Rc::new(scroll));
}

pub(crate) struct AutoscrollLayer {
    active: Option<Active>,
    /// Set while the layer sends its own wheel events, so they aren't taken
    /// for the user's wheel (which stops autoscroll).
    replaying: Rc<Cell<bool>>,
}

struct Active {
    scroll: Autoscroll,
    /// The view that latched the vertical axis, if any: the wheel goes to
    /// whatever is under the anchor otherwise.
    vertical: Option<ScrollFn>,
    _tick: Task<()>,
}

impl AutoscrollLayer {
    pub(crate) fn new() -> Self {
        Self { active: None, replaying: Rc::new(Cell::new(false)) }
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Ends autoscroll. Returns whether one was running.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.active.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    /// Returns whether the press is used up (it must not reach the views).
    fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The views' capture listeners ran before this one.
        let vertical = cx.default_global::<Offered>().0.take();
        // Any press ends a running autoscroll, and does nothing else.
        if self.stop(cx) {
            return true;
        }
        if event.button != MouseButton::Middle {
            return false;
        }
        let now = cx.background_executor().now();
        let scroll = Autoscroll::start(vec2(event.position), now);
        self.active = Some(Active { scroll, vertical, _tick: self.tick(window, cx) });
        cx.notify();
        true
    }

    /// Returns whether the release is used up.
    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        if event.button != MouseButton::Middle {
            return false;
        }
        let now = cx.background_executor().now();
        let Some(active) = &mut self.active else {
            return false;
        };
        if active.scroll.released(now) == Release::Stop {
            self.stop(cx);
        }
        true
    }

    fn mouse_moved(&mut self, event: &MouseMoveEvent) {
        if let Some(active) = &mut self.active {
            active.scroll.pointer_moved(vec2(event.position));
        }
    }

    /// Scrolls by the elapsed time's step every [`TICK`], until autoscroll ends
    /// (dropping the task stops it too).
    fn tick(&self, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        let replaying = Rc::clone(&self.replaying);
        cx.spawn_in(window, async move |this, cx| {
            let mut last = cx.background_executor().now();
            loop {
                cx.background_executor().timer(TICK).await;
                let now = cx.background_executor().now();
                let elapsed = now.saturating_duration_since(last);
                last = now;
                let Ok(Some((scroll, vertical))) = this.read_with(cx, |this, _| {
                    this.active
                        .as_ref()
                        .map(|active| (active.scroll.clone(), active.vertical.clone()))
                }) else {
                    break;
                };
                let step = scroll.step(elapsed);
                if step == Vec2::default() {
                    continue;
                }
                // Not inside an update of this view or the app: the wheel
                // listeners of every view under the anchor run from here.
                let sent = cx.update(|window, cx| {
                    let step = match &vertical {
                        Some(scroll) => {
                            scroll(step.y, cx);
                            Vec2::new(step.x, 0.)
                        }
                        None => step,
                    };
                    replaying.set(true);
                    replay(&scroll, step, window, cx);
                    replaying.set(false);
                });
                if sent.is_err() {
                    break;
                }
            }
        })
    }
}

/// Sends `step` as wheel events at the anchor, then puts the pointer back
/// where it really is: a wheel event moves the window's idea of the pointer,
/// and hover would otherwise stick to the anchor.
fn replay(scroll: &Autoscroll, step: Vec2, window: &mut Window, cx: &mut App) {
    let anchor = scroll.anchor();
    let modifiers = window.modifiers();
    // One axis per event: the deck locks a wheel gesture to its main axis and
    // would drop the other one. Each event starts its own gesture, so the
    // lock never carries over. A `list()` adds up a frame's wheel deltas only
    // while their signs match (and replaces them otherwise), so each event's
    // zero takes the other axis's sign: two ticks before a frame then add up
    // instead of the second undoing the first. A wheel's positive delta
    // scrolls back (up / left), hence the minus.
    let (x, y) = (-step.x, -step.y);
    let events =
        [(x != 0.).then_some((x, 0f32.copysign(y))), (y != 0.).then_some((0f32.copysign(x), y))];
    for (dx, dy) in events.into_iter().flatten() {
        let delta = point(px(dx), px(dy));
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: point(px(anchor.x), px(anchor.y)),
                delta: ScrollDelta::Pixels(delta),
                modifiers,
                touch_phase: TouchPhase::Started,
            }),
            cx,
        );
    }
    let pointer = scroll.pointer();
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(pointer.x), px(pointer.y)),
            pressed_button: scroll.is_held().then_some(MouseButton::Middle),
            modifiers,
        }),
        cx,
    );
}

fn vec2(p: gpui_kit::Point<gpui_kit::Pixels>) -> Vec2 {
    Vec2::new(f32::from(p.x), f32::from(p.y))
}

impl Render for AutoscrollLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let replaying = Rc::clone(&self.replaying);
        // Capture phase, so the press is seen (and used up) before any view,
        // even one under an occluding sheet or dialog.
        let listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let weak = this.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let used = weak.update(cx, |this, cx| this.mouse_down(event, window, cx));
                    if used.unwrap_or(false) {
                        cx.stop_propagation();
                    }
                });
                let weak = this.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if weak.update(cx, |this, cx| this.mouse_up(event, cx)).unwrap_or(false) {
                        cx.stop_propagation();
                    }
                });
                let weak = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        weak.update(cx, |this, _| this.mouse_moved(event)).ok();
                    }
                });
                let weak = this.clone();
                let replaying = Rc::clone(&replaying);
                window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _, cx| {
                    // The user's own wheel takes over.
                    if phase == DispatchPhase::Capture && !replaying.get() {
                        weak.update(cx, |this, cx| this.stop(cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let marker = self.active.as_ref().map(|active| {
            let theme = cx.theme();
            let anchor = active.scroll.anchor();
            div()
                .debug_selector(|| "autoscroll-anchor".into())
                .absolute()
                .left(px(anchor.x - MARKER / 2.))
                .top(px(anchor.y - MARKER / 2.))
                .size(px(MARKER))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background.opacity(0.85))
                .shadow_md()
                .child(div().size(px(MARKER_DOT)).rounded_full().bg(theme.muted_foreground))
        });
        div().absolute().top_0().left_0().size_full().child(listeners).children(marker)
    }
}
