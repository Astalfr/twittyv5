//! The deck dialog, for a new deck (from the sidebar's **+**) or an existing
//! one (from its pencil in the expanded sidebar): its name and its icon, and
//! removing it. Edits to an existing deck apply right away; a new deck is only
//! made when **Create** is pressed.

use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, assets::IconName, component::button::*, component::input::*,
    component::*, div, prelude::*, px,
};

use crate::{state::deck::DeckIcon, ui::button::TextButton};

/// Icons per row in the picker.
const ICONS_PER_ROW: usize = 10;

/// The Lucide icon drawn for `icon`.
pub(crate) fn icon(icon: DeckIcon) -> IconName {
    match icon {
        DeckIcon::Grid => IconName::LayoutGrid,
        DeckIcon::House => IconName::House,
        DeckIcon::Star => IconName::Star,
        DeckIcon::Bell => IconName::Bell,
        DeckIcon::Globe => IconName::Globe,
        DeckIcon::Zap => IconName::Zap,
        DeckIcon::Flame => IconName::Flame,
        DeckIcon::Heart => IconName::Heart,
        DeckIcon::Bookmark => IconName::Bookmark,
        DeckIcon::Briefcase => IconName::Briefcase,
        DeckIcon::Code => IconName::Code,
        DeckIcon::Gamepad => IconName::Gamepad2,
        DeckIcon::Music => IconName::Music,
        DeckIcon::Film => IconName::Film,
        DeckIcon::Newspaper => IconName::Newspaper,
        DeckIcon::Trophy => IconName::Trophy,
        DeckIcon::Rocket => IconName::Rocket,
        DeckIcon::Coffee => IconName::Coffee,
        DeckIcon::Lightbulb => IconName::Lightbulb,
        DeckIcon::Camera => IconName::Camera,
    }
}

/// Which deck the dialog is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckTarget {
    /// A deck still to be made.
    New,
    /// Deck `index`; `removable` unless it is the only one.
    Existing { index: usize, removable: bool },
}

/// What the dialog reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum DeckSettingsEvent {
    /// Deck `index` got a new name (as typed: the deck trims it, and keeps
    /// its old name for blank text).
    Renamed(usize, String),
    /// Deck `index` got a new icon.
    IconChanged(usize, DeckIcon),
    /// Remove deck `index`.
    Remove(usize),
    /// Make a new deck (the name as typed; blank gets "Deck N").
    Create(String, DeckIcon),
    Dismissed,
}

impl EventEmitter<DeckSettingsEvent> for DeckSettingsView {}

/// The deck dialog's view.
pub struct DeckSettingsView {
    target: DeckTarget,
    /// The name as typed.
    text: String,
    icon: DeckIcon,
    name: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl std::fmt::Debug for DeckSettingsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeckSettingsView")
            .field("target", &self.target)
            .field("icon", &self.icon)
            .finish_non_exhaustive()
    }
}

impl DeckSettingsView {
    /// The dialog for `target`, starting from `name` and `icon`. For a new
    /// deck, `name` is only the placeholder: the name a blank field gets.
    pub fn new(
        target: DeckTarget,
        name: &str,
        icon: DeckIcon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = name.to_owned();
        let text = match target {
            DeckTarget::New => String::new(),
            DeckTarget::Existing { .. } => name.to_owned(),
        };
        let initial = text.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder(placeholder, window, cx);
            state.set_value(initial, window, cx);
            state
        });
        let subs =
            vec![cx.subscribe_in(&input, window, |this, input, e: &InputEvent, _, cx| match e {
                InputEvent::Change => {
                    let text = input.read(cx).value();
                    this.set_name(&text, cx);
                }
                InputEvent::PressEnter { .. } => this.confirm(cx),
                _ => {}
            })];
        input.update(cx, |s, cx| s.focus(window, cx));
        Self { target, text, icon, name: input, _subs: subs }
    }

    pub fn target(&self) -> DeckTarget {
        self.target
    }

    pub fn icon(&self) -> DeckIcon {
        self.icon
    }

    /// Sets the name; an existing deck is renamed at once.
    pub fn set_name(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        if let DeckTarget::Existing { index, .. } = self.target {
            cx.emit(DeckSettingsEvent::Renamed(index, self.text.clone()));
        }
        cx.notify();
    }

    /// Picks `icon`; an existing deck gets it at once.
    pub fn set_icon(&mut self, icon: DeckIcon, cx: &mut Context<Self>) {
        if icon != self.icon {
            self.icon = icon;
            if let DeckTarget::Existing { index, .. } = self.target {
                cx.emit(DeckSettingsEvent::IconChanged(index, icon));
            }
        }
        cx.notify();
    }

    /// The main button (and Enter): creates the new deck, or closes.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        cx.emit(match self.target {
            DeckTarget::New => DeckSettingsEvent::Create(self.text.clone(), self.icon),
            DeckTarget::Existing { .. } => DeckSettingsEvent::Dismissed,
        });
    }

    /// Asks the app to remove the deck (never the only one).
    pub fn remove(&mut self, cx: &mut Context<Self>) {
        if let DeckTarget::Existing { index, removable: true } = self.target {
            cx.emit(DeckSettingsEvent::Remove(index));
        }
    }
}

impl Render for DeckSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let section =
            |title: &'static str| div().pt_2().text_sm().font_weight(FontWeight::BOLD).child(title);
        let rows = DeckIcon::ALL.chunks(ICONS_PER_ROW).enumerate().map(|(r, row)| {
            h_flex().gap_1().children(row.iter().enumerate().map(|(c, &choice)| {
                Button::new(("deck-icon", r * ICONS_PER_ROW + c))
                    .debug_selector(move || format!("deck-icon-{}", choice.key()))
                    .ghost()
                    .icon(Icon::new(icon(choice)).size(px(18.)))
                    .selected(choice == self.icon)
                    .w(px(32.))
                    .h(px(32.))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_icon(choice, cx)))
            }))
        });
        let (title, confirm) = match self.target {
            DeckTarget::New => ("New deck", "Create"),
            DeckTarget::Existing { .. } => ("Edit deck", "Done"),
        };
        // Cancel for a new deck, Remove for an existing one.
        let secondary = match self.target {
            DeckTarget::New => Button::new("deck-cancel")
                .debug_selector(|| "deck-cancel".into())
                .ghost()
                .text_button(Size::Medium)
                .label("Cancel")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(DeckSettingsEvent::Dismissed))),
            DeckTarget::Existing { removable, .. } => Button::new("deck-remove")
                .debug_selector(|| "deck-remove".into())
                .danger()
                .icon(IconName::Trash)
                .text_button(Size::Medium)
                .label("Remove deck")
                .disabled(!removable)
                .when(!removable, |b| b.tooltip("The only deck can't be removed"))
                .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
        };
        v_flex()
            .id("deck-settings")
            .w(px(420.))
            .gap_3()
            .p_6()
            .rounded_2xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.foreground)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child(title))
            .child(section("Name"))
            .child(Input::new(&self.name))
            .child(section("Icon"))
            .child(v_flex().gap_1().children(rows))
            .child(
                h_flex().justify_between().pt_2().child(secondary).child(
                    Button::new("deck-settings-confirm")
                        .debug_selector(|| "deck-settings-confirm".into())
                        .primary()
                        .text_button(Size::Medium)
                        .label(confirm)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};

    fn open(
        target: DeckTarget,
        cx: &mut TestAppContext,
    ) -> (Entity<DeckSettingsView>, Arc<Mutex<Vec<DeckSettingsEvent>>>, &mut VisualTestContext)
    {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            DeckSettingsView::new(target, "Work", DeckIcon::Grid, window, cx)
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &DeckSettingsEvent, _| {
                seen.lock().unwrap().push(e.clone())
            })
            .detach();
        });
        cx.run_until_parked();
        (view, events, cx)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    const DECK_2: DeckTarget = DeckTarget::Existing { index: 2, removable: true };

    #[gpui_kit::test]
    fn typing_renames_and_a_picked_icon_is_reported_once(cx: &mut TestAppContext) {
        let (view, events, cx) = open(DECK_2, cx);
        cx.simulate_input("!");
        click(cx, "deck-icon-rocket");
        click(cx, "deck-icon-rocket");
        assert_eq!(view.read_with(cx, |v, _| v.icon()), DeckIcon::Rocket);
        assert_eq!(
            *events.lock().unwrap(),
            [
                DeckSettingsEvent::Renamed(2, "Work!".into()),
                DeckSettingsEvent::IconChanged(2, DeckIcon::Rocket),
            ],
            "picking the same icon again is no change"
        );
        click(cx, "deck-settings-confirm");
        assert_eq!(events.lock().unwrap().last(), Some(&DeckSettingsEvent::Dismissed));
    }

    #[gpui_kit::test]
    fn a_new_deck_is_only_made_on_create(cx: &mut TestAppContext) {
        let (_, events, cx) = open(DeckTarget::New, cx);
        assert!(cx.debug_bounds("deck-remove").is_none(), "nothing to remove yet");
        cx.simulate_input("News");
        click(cx, "deck-icon-newspaper");
        assert!(events.lock().unwrap().is_empty(), "nothing made while editing");
        click(cx, "deck-settings-confirm");
        assert_eq!(
            *events.lock().unwrap(),
            [DeckSettingsEvent::Create("News".into(), DeckIcon::Newspaper)]
        );
    }

    #[gpui_kit::test]
    fn cancel_makes_no_deck(cx: &mut TestAppContext) {
        let (_, events, cx) = open(DeckTarget::New, cx);
        click(cx, "deck-cancel");
        assert_eq!(*events.lock().unwrap(), [DeckSettingsEvent::Dismissed]);
    }

    #[gpui_kit::test]
    fn remove_asks_the_app_to_remove_the_deck(cx: &mut TestAppContext) {
        let (_, events, cx) = open(DECK_2, cx);
        click(cx, "deck-remove");
        assert_eq!(*events.lock().unwrap(), [DeckSettingsEvent::Remove(2)]);
    }

    #[gpui_kit::test]
    fn the_only_deck_cannot_be_removed(cx: &mut TestAppContext) {
        let (_, events, cx) = open(DeckTarget::Existing { index: 0, removable: false }, cx);
        click(cx, "deck-remove");
        assert!(events.lock().unwrap().is_empty(), "the only deck stays");
    }

    #[test]
    fn every_deck_icon_has_its_own_picture() {
        let mut paths: Vec<_> = DeckIcon::ALL.iter().map(|&i| icon(i).path()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), DeckIcon::ALL.len());
    }
}
