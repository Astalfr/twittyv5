//! A column's settings dialog: its name, width, and how it announces new
//! tweets. Every change applies to the column right away.

use gpui_kit::{
    App, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, component::button::*, component::input::*, component::slider::*,
    component::switch::Switch, component::*, div, prelude::*, px,
};
use std::sync::Arc;

use twitty_model::{FeedSource, Profile};

use crate::{
    sound,
    state::alerts::Announcement,
    state::deck::{ColumnId, ColumnSettings, ColumnWidth, Volume},
    ui::button::TextButton,
    ui::column,
};

/// Volume slider granularity, in percent.
const VOLUME_STEP: f32 = 5.0;

/// The text of the notification the Test button sends.
pub const TEST_NOTIFICATION: &str =
    "Test notification: new tweets in this column show up like this.";

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnSettingsEvent {
    /// A slider is being dragged: show the new settings, but don't save yet.
    Preview(ColumnId, ColumnSettings),
    /// A finished change, to show and save.
    Changed(ColumnId, ColumnSettings),
    Dismissed,
}

impl EventEmitter<ColumnSettingsEvent> for ColumnSettingsView {}

/// The column settings dialog's view.
pub struct ColumnSettingsView {
    id: ColumnId,
    /// The column's source, whose title stands in for a blank name.
    source: FeedSource,
    /// The signed-in account, whom the test notification is from.
    viewer: Option<Arc<Profile>>,
    settings: ColumnSettings,
    name: Entity<InputState>,
    width: Entity<SliderState>,
    volume: Entity<SliderState>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for ColumnSettingsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnSettingsView")
            .field("id", &self.id)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl ColumnSettingsView {
    pub fn new(
        id: ColumnId,
        source: &FeedSource,
        viewer: Option<Arc<Profile>>,
        settings: ColumnSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = source.title();
        let current = settings.name.clone().unwrap_or_default();
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder(placeholder, window, cx);
            state.set_value(current, window, cx);
            state
        });
        let width = cx.new(|_| {
            SliderState::new()
                // `min` clamps against the current max (100), so max first.
                .max(f32::from(ColumnWidth::MAX))
                .min(f32::from(ColumnWidth::MIN))
                .step(f32::from(ColumnWidth::STEP))
                .default_value(settings.width.px())
        });
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(f32::from(Volume::MAX))
                .step(VOLUME_STEP)
                .default_value(f32::from(settings.alerts.volume.percent()))
        });
        let subs = vec![
            cx.subscribe_in(&name, window, |this, input, e: &InputEvent, _, cx| match e {
                InputEvent::Change => {
                    let text = input.read(cx).value();
                    this.set_name(&text, cx);
                }
                InputEvent::PressEnter { .. } => cx.emit(ColumnSettingsEvent::Dismissed),
                _ => {}
            }),
            cx.subscribe(&width, |this, _, e: &SliderEvent, cx| match e {
                SliderEvent::Change(value) => this.set_width(value.start(), false, cx),
                SliderEvent::Release(value) => this.set_width(value.start(), true, cx),
            }),
            cx.subscribe(&volume, |this, _, e: &SliderEvent, cx| match e {
                SliderEvent::Change(value) => this.set_volume(value.start(), false, cx),
                SliderEvent::Release(value) => {
                    this.set_volume(value.start(), true, cx);
                    // Let the user hear the level they picked.
                    sound::play(this.settings.alerts.volume, cx);
                }
            }),
        ];
        name.update(cx, |s, cx| s.focus(window, cx));
        Self { id, source: source.clone(), viewer, settings, name, width, volume, _subs: subs }
    }

    pub fn id(&self) -> ColumnId {
        self.id
    }

    pub fn settings(&self) -> &ColumnSettings {
        &self.settings
    }

    /// Reports the settings: as a preview, or as a change to save.
    fn report(&self, save: bool, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        cx.emit(if save {
            ColumnSettingsEvent::Changed(self.id, settings)
        } else {
            ColumnSettingsEvent::Preview(self.id, settings)
        });
        cx.notify();
    }

    /// Renames the column; blank text goes back to the source's title.
    pub fn set_name(&mut self, text: &str, cx: &mut Context<Self>) {
        let before = self.settings.name.clone();
        self.settings.set_name(text);
        if self.settings.name != before {
            self.report(true, cx);
        }
    }

    /// Sets the width to `px` (clamped); `save` once the slider is released.
    pub fn set_width(&mut self, px: f32, save: bool, cx: &mut Context<Self>) {
        self.settings.width = ColumnWidth::new(px);
        self.report(save, cx);
    }

    /// Sets the sound's volume to `percent`; `save` once the slider is released.
    pub fn set_volume(&mut self, percent: f32, save: bool, cx: &mut Context<Self>) {
        self.settings.alerts.volume = Volume::new(percent);
        self.report(save, cx);
    }

    /// Turns the sound on or off. Turning it on plays it once.
    pub fn set_sound(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.alerts.sound = on;
        if on {
            sound::play(self.settings.alerts.volume, cx);
        }
        self.report(true, cx);
    }

    /// Turns the system notification on or off.
    pub fn set_desktop(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings.alerts.desktop = on;
        self.report(true, cx);
    }

    /// Sends a sample notification the way new tweets will, switched on or
    /// not: from the signed-in account, as if they had tweeted.
    pub fn test_notification(&self, cx: &mut Context<Self>) {
        let news = Announcement {
            count: 1,
            from: self.viewer.clone(),
            lines: vec![TEST_NOTIFICATION.to_owned()],
        };
        column::show_notification(self.id, &self.settings.title(&self.source), &news, cx);
    }
}

/// A label with its current value on the right.
fn labelled(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_between()
        .text_sm()
        .child(div().font_weight(FontWeight::MEDIUM).child(label))
        .child(div().text_color(cx.theme().muted_foreground).child(value))
}

/// The system notification switch's label, named after the platform's.
const DESKTOP_LABEL: &str = if cfg!(windows) {
    "Show a Windows notification for new tweets"
} else {
    "Show a system notification for new tweets"
};

impl Render for ColumnSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let weak = cx.entity().downgrade();
        let alerts = self.settings.alerts;
        let section =
            |title: &'static str| div().pt_2().text_sm().font_weight(FontWeight::BOLD).child(title);
        v_flex()
            .id("column-settings")
            .w(px(420.))
            .gap_3()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Column settings"))
            .child(section("Name"))
            .child(Input::new(&self.name))
            .child(section("Width"))
            .child(
                v_flex()
                    .gap_2()
                    .child(labelled("Column width", format!("{} px", self.settings.width.px()), cx))
                    .child(Slider::new(&self.width).accessibility_label("Column width")),
            )
            .child(section("New tweets"))
            .child(
                Switch::new("column-sound").checked(alerts.sound).label("Play a sound").on_click({
                    let weak = weak.clone();
                    move |on, _, cx| {
                        weak.update(cx, |this, cx| this.set_sound(*on, cx)).ok();
                    }
                }),
            )
            .when(alerts.sound, |d| {
                d.child(
                    v_flex()
                        .debug_selector(|| "column-volume".into())
                        .gap_2()
                        .pl_6()
                        .child(labelled("Volume", format!("{}%", alerts.volume.percent()), cx))
                        .child(Slider::new(&self.volume).accessibility_label("Sound volume")),
                )
            })
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child(
                        Switch::new("column-desktop")
                            .checked(alerts.desktop)
                            .label(DESKTOP_LABEL)
                            .on_click(move |on, _, cx| {
                                weak.update(cx, |this, cx| this.set_desktop(*on, cx)).ok();
                            }),
                    )
                    .child(
                        Button::new("column-test-notification")
                            .debug_selector(|| "column-test-notification".into())
                            .text_button(Size::XSmall)
                            .label("Test")
                            .tooltip("Send a test notification")
                            .on_click(cx.listener(|this, _, _, cx| this.test_notification(cx))),
                    ),
            )
            .child(
                h_flex().justify_end().pt_2().child(
                    Button::new("column-settings-done")
                        .primary()
                        .text_button(Size::Medium)
                        .label("Done")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(ColumnSettingsEvent::Dismissed)),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sound::RecordingSound, state::deck::ColumnAlerts};
    use gpui_kit::TestAppContext;
    use std::sync::{Arc, Mutex};

    fn open(
        settings: ColumnSettings,
        cx: &mut TestAppContext,
    ) -> (
        Entity<ColumnSettingsView>,
        Arc<Mutex<Vec<ColumnSettingsEvent>>>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            ColumnSettingsView::new(ColumnId(3), &FeedSource::Bookmarks, None, settings, window, cx)
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &ColumnSettingsEvent, _| {
                seen.lock().unwrap().push(e.clone())
            })
            .detach();
        });
        cx.run_until_parked();
        (view, events, cx)
    }

    #[gpui_kit::test]
    fn edits_are_reported_and_drags_only_previewed(cx: &mut TestAppContext) {
        let (view, events, cx) = open(ColumnSettings::default(), cx);
        view.update(cx, |v, cx| {
            v.set_width(512.3, false, cx);
            v.set_width(1000., true, cx);
            v.set_name("  Saved ", cx);
            v.set_name("Saved", cx);
        });
        cx.run_until_parked();
        let events = events.lock().unwrap().clone();
        let widths: Vec<_> = events
            .iter()
            .map(|e| match e {
                ColumnSettingsEvent::Preview(_, s) => ("preview", s.width.px()),
                ColumnSettingsEvent::Changed(_, s) => ("save", s.width.px()),
                ColumnSettingsEvent::Dismissed => ("dismissed", 0.),
            })
            .collect();
        assert_eq!(
            widths,
            [("preview", 512.), ("save", f32::from(ColumnWidth::MAX)), ("save", 640.)],
            "the same name twice is reported once"
        );
        let ColumnSettingsEvent::Changed(id, last) = events.last().unwrap() else {
            panic!("expected a change");
        };
        assert_eq!(*id, ColumnId(3));
        assert_eq!(last.name.as_deref(), Some("Saved"));
    }

    #[gpui_kit::test]
    fn the_test_button_sends_a_notification(cx: &mut TestAppContext) {
        // Like Windows, GPUI only tweets notifications for an identified app.
        cx.update(|cx| cx.set_app_identity("Twitty.Test", "Twitty"));
        let (view, events, cx) = open(ColumnSettings::default(), cx);
        let button = cx.debug_bounds("column-test-notification").expect("the Test button is drawn");
        cx.simulate_click(button.center(), gpui_kit::Modifiers::default());
        cx.run_until_parked();

        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1, "sent even while the switch is off");
        assert_eq!(shown[0].tag.as_ref(), "column-3", "replaces the column's own toast");
        assert_eq!(shown[0].title.as_ref(), "Bookmarks", "the column's title, signed out");
        assert_eq!(shown[0].body.as_ref(), TEST_NOTIFICATION);
        assert!(
            !view.read_with(cx, |v, _| v.settings().alerts.desktop),
            "the setting is untouched"
        );
        assert!(events.lock().unwrap().is_empty(), "nothing to save");

        view.update(cx, |v, cx| v.set_name("Saved", cx));
        view.update(cx, |v, cx| v.test_notification(cx));
        assert_eq!(cx.shown_system_notifications()[1].title.as_ref(), "Saved");
    }

    #[gpui_kit::test]
    fn the_test_notification_is_from_the_signed_in_account(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_app_identity("Twitty.Test", "Twitty");
        });
        let me = Arc::new(Profile::new(twitty_model::UserId::new("1"), "me", "My Name"));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let settings = ColumnSettings::default();
            ColumnSettingsView::new(
                ColumnId(3),
                &FeedSource::Bookmarks,
                Some(me),
                settings,
                window,
                cx,
            )
        });
        view.update(cx, |v, cx| v.test_notification(cx));
        let shown = cx.shown_system_notifications();
        assert_eq!(shown[0].title.as_ref(), "Bookmarks - My Name @me", "titled like a tweet's");
        assert_eq!(shown[0].body.as_ref(), TEST_NOTIFICATION);
    }

    #[gpui_kit::test]
    fn the_volume_slider_only_shows_with_the_sound(cx: &mut TestAppContext) {
        let sound = cx.update(RecordingSound::install);
        let (view, _, cx) = open(ColumnSettings::default(), cx);
        assert!(cx.debug_bounds("column-volume").is_none());

        view.update(cx, |v, cx| v.set_sound(true, cx));
        cx.run_until_parked();
        assert_eq!(sound.played(), [Volume::default()], "turning it on plays it once");
        assert!(cx.debug_bounds("column-volume").is_some());

        view.update(cx, |v, cx| {
            v.set_volume(42., true, cx);
            v.set_desktop(true, cx);
        });
        let alerts = view.read_with(cx, |v, _| v.settings().alerts);
        assert_eq!(alerts, ColumnAlerts { sound: true, volume: Volume::from(42), desktop: true });

        view.update(cx, |v, cx| v.set_sound(false, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("column-volume").is_none());
    }
}
