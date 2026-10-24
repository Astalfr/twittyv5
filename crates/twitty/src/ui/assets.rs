//! Icon assets: GPUI Kit's default component icons plus the extra Lucide icons Twitty uses.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    TwittyIcons,
    [
        House,
        Search,
        Bell,
        Mail,
        List,
        Plus,
        PenLine,
        MessageCircle,
        Repeat2,
        Heart,
        ChartNoAxesColumn,
        Bookmark,
        Share,
        Settings,
        Ellipsis,
        BadgeCheck,
        Play,
        X,
        ArrowUp,
        RefreshCw,
        ChevronLeft,
        ChevronRight,
        MoveHorizontal,
        SunMoon,
        User,
        CircleAlert,
        CircleUser,
        LogIn,
        LogOut,
    ]
);

/// Path of the Twitty logo shown in the rail.
pub const LOGO: &str = "brand/twitty.png";

/// The app's asset source.
#[derive(Debug, Clone, Copy)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == LOGO {
            return Ok(Some(Cow::Borrowed(include_bytes!("../../resources/twitty-logo.png"))));
        }
        if let Some(bytes) = TwittyIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(TwittyIcons.list(path)?);
        if LOGO.starts_with(path) {
            paths.push(LOGO.into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    #[test]
    fn every_used_icon_resolves() {
        for name in [
            IconName::House,
            IconName::Heart,
            IconName::Repeat2,
            IconName::ChartNoAxesColumn,
            IconName::BadgeCheck,
            IconName::SunMoon,
            IconName::CircleAlert,
            IconName::CircleUser,
            IconName::LogIn,
            IconName::LogOut,
            // The title bar's window buttons.
            IconName::WindowMinimize,
            IconName::WindowMaximize,
            IconName::WindowRestore,
            IconName::WindowClose,
        ] {
            let path = name.path();
            assert!(Assets.load(&path).unwrap().is_some(), "missing icon {path}");
        }
    }

    #[test]
    fn the_logo_is_a_png() {
        let bytes = Assets.load(LOGO).unwrap().expect("logo is embedded");
        assert_eq!(bytes.get(1..4), Some(&b"PNG"[..]), "PNG signature");
        assert!(Assets.list("brand/").unwrap().iter().any(|p| p.as_ref() == LOGO));
    }
}
