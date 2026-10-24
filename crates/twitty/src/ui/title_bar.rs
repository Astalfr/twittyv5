//! The window's title bar, in place of the system one.
//!
//! A copy of GPUI Kit's `TitleBar` with our own sizes: its height and its
//! window buttons' width are private constants there. It behaves the same:
//! dragging it moves the window and, on Windows, the buttons are real caption
//! buttons (Snap layouts work).

use gpui_kit::{
    AnyElement, App, Decorations, Hsla, IntoElement, MouseButton, ParentElement, Pixels, Render,
    RenderOnce, Styled, TestSupportExt as _, Window, WindowControlArea, assets::IconName,
    component::*, div, prelude::*, px,
};

/// The bar's height.
pub const HEIGHT: Pixels = px(39.);

/// Room on each side of a window button's icon (GPUI Kit's is 10px).
const CONTROL_PADDING: Pixels = px(20.);

/// The drawn size of a window button's icon (`Icon::small`).
const CONTROL_ICON: Pixels = px(14.);

/// The title bar: a drag area with the window buttons on its right (none on
/// macOS, which draws its own).
#[derive(Debug, IntoElement)]
pub struct TitleBar {
    background: Hsla,
}

impl TitleBar {
    /// A bar filled with `background`.
    pub fn new(background: Hsla) -> Self {
        Self { background }
    }
}

/// Whether a press on the bar should start a window move (macOS / Linux,
/// where the app moves the window itself).
struct DragState {
    should_move: bool,
}

impl Render for DragState {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        div()
    }
}

impl RenderOnce for TitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_linux = cfg!(target_os = "linux");
        let is_macos = cfg!(target_os = "macos");
        let state = window.use_state(cx, |_, _| DragState { should_move: false });
        div()
            .id("title-bar")
            .flex_none()
            .flex()
            .items_center()
            .h(HEIGHT)
            .bg(self.background)
            .when(is_linux, |this| this.on_double_click(|_, window, _| window.zoom_window()))
            .when(is_macos, |this| {
                this.on_double_click(|_, window, _| window.titlebar_double_click())
            })
            // After `on_double_click`, which only `Stateful` has.
            .test_support()
            .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| {
                state.should_move = false;
            }))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| state.should_move = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| state.should_move = false),
            )
            .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
                if state.should_move {
                    state.should_move = false;
                    window.start_window_move();
                }
            }))
            .child(div().h_full().flex_1().window_control_area(WindowControlArea::Drag))
            .child(window_controls(window))
    }
}

/// Minimise, maximise / restore and close, as far as the platform offers them.
fn window_controls(window: &Window) -> AnyElement {
    if cfg!(target_os = "macos") {
        return div().into_any_element();
    }
    // Under server-side decorations the window manager draws its own buttons.
    if cfg!(target_os = "linux")
        && !matches!(window.window_decorations(), Decorations::Client { .. })
    {
        return div().into_any_element();
    }
    let supported = window.window_controls();
    h_flex()
        .id("window-controls")
        .test_support()
        .flex_none()
        .h_full()
        .when(supported.minimize, |this| this.child(Control::Minimize))
        .when(supported.maximize, |this| {
            this.child(if window.is_maximized() { Control::Restore } else { Control::Maximize })
        })
        .child(Control::Close)
        .into_any_element()
}

#[derive(IntoElement, Clone, Copy)]
enum Control {
    Minimize,
    Restore,
    Maximize,
    Close,
}

impl Control {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Restore => "restore",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Minimize => IconName::WindowMinimize,
            Self::Restore => IconName::WindowRestore,
            Self::Maximize => IconName::WindowMaximize,
            Self::Close => IconName::WindowClose,
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Restore | Self::Maximize => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

impl RenderOnce for Control {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (hover_bg, hover_fg, active_bg) = match self {
            Self::Close => (theme.danger, theme.danger_foreground, theme.danger_active),
            _ => (theme.secondary_hover, theme.secondary_foreground, theme.secondary_active),
        };
        div()
            .id(self.id())
            .test_support()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(CONTROL_ICON + CONTROL_PADDING * 2.)
            .h_full()
            .text_color(theme.foreground)
            .hover(|style| style.bg(hover_bg).text_color(hover_fg))
            .active(|style| style.bg(active_bg).text_color(hover_fg))
            // Windows handles clicks on caption areas itself.
            .when(cfg!(target_os = "windows"), |this| this.window_control_area(self.area()))
            .when(cfg!(target_os = "linux"), |this| {
                this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match self {
                        Self::Minimize => window.minimize_window(),
                        Self::Restore | Self::Maximize => window.zoom_window(),
                        Self::Close => window.remove_window(),
                    }
                })
            })
            .child(Icon::new(self.icon()).small())
    }
}
