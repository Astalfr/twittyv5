//! Twitty's X-like themes (Lights out, Dim, Light), registered with GPUI Kit.

use gpui_kit::{
    App, SharedString, Window,
    component::{Theme, ThemeRegistry, scroll::ScrollbarMode},
};

const THEMES: &str = include_str!("../../themes/x.json");

/// Theme names, in the order the settings page lists them.
pub const NAMES: [&str; 3] = ["Lights out", "Dim", "Light"];

/// Registers the bundled themes and activates `name`.
pub fn init(name: &str, cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEMES) {
        tracing::error!("bundled themes failed to load: {err:#}");
    }
    apply(name, None, cx);
    // Columns and the deck always show their scrollbars, like X Pro.
    Theme::set_scrollbar_mode(ScrollbarMode::Always, cx);
}

/// Activates the theme called `name`, if it exists.
pub fn apply(name: &str, window: Option<&mut Window>, cx: &mut App) {
    let key = SharedString::from(name.to_owned());
    let Some(config) = ThemeRegistry::global(cx).themes().get(&key).cloned() else {
        tracing::warn!("unknown theme {name}");
        return;
    };
    Theme::global_mut(cx).apply_config(&config);
    if let Some(window) = window {
        window.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_theme_file_parses_and_has_all_names() {
        let json: serde_json::Value = serde_json::from_str(THEMES).unwrap();
        let names: Vec<&str> = json["themes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, NAMES);
    }

    #[gpui_kit::test]
    fn init_applies_theme_and_always_shows_scrollbars(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init("Dim", cx);
            assert_eq!(Theme::global(cx).scrollbar_mode, ScrollbarMode::Always);
            assert_eq!(Theme::global(cx).theme_name().as_ref(), "Dim");
        });
    }
}
