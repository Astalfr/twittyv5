//! Twitty's X-like themes (Lights out, Dim, Light), registered with GPUI Kit.

use gpui_kit::{
    App, SharedString, Window,
    component::{Theme, ThemeRegistry},
};

const THEMES: &str = include_str!("../../themes/x.json");

/// Theme names, in the order the toggle cycles through them.
pub const NAMES: [&str; 3] = ["Lights out", "Dim", "Light"];

/// Registers the bundled themes and activates `name`.
pub fn init(name: &str, cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEMES) {
        tracing::error!("bundled themes failed to load: {err:#}");
    }
    apply(name, None, cx);
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

/// The theme after `current` in [`NAMES`].
pub fn next(current: &str) -> &'static str {
    let ix = NAMES.iter().position(|n| *n == current).map_or(0, |i| (i + 1) % NAMES.len());
    NAMES[ix]
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

    #[test]
    fn next_cycles() {
        assert_eq!(next("Lights out"), "Dim");
        assert_eq!(next("Light"), "Lights out");
        assert_eq!(next("unknown"), "Lights out");
    }
}
