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
        Pause,
        RotateCcw,
        Volume2,
        VolumeX,
        X,
        ArrowUp,
        RefreshCw,
        ChevronLeft,
        ChevronRight,
        SlidersHorizontal,
        SunMoon,
        User,
        CircleAlert,
        CircleUser,
        LogIn,
        LogOut,
    ]
);

/// Path of the Twitty logo shown in the title bar.
pub const LOGO: &str = "brand/twitty.png";

/// Size the logo is drawn at, in logical pixels. GPUI samples images
/// bilinearly with no mipmaps, so a PNG much larger than its drawn size comes
/// out jagged: the title bar draws the (256px) logo through
/// `remote_image::asset`, which resamples it to the device size first.
pub const LOGO_SIZE: f32 = 18.;

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

    #[test]
    fn the_logo_is_resampled_to_its_drawn_size_at_any_scale() {
        use crate::{state::image_scale::BoxPx, ui::remote_image::decode};
        let bytes = Assets.load(LOGO).unwrap().expect("logo is embedded");
        let dim = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(dim(16), dim(20), "square");
        // Unshrunk, GPUI would skip source pixels (no mipmaps) and alias.
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let target = BoxPx::bucket(LOGO_SIZE, LOGO_SIZE, scale).unwrap();
            assert!(dim(16) > target.w * 2, "the source is sharp enough at {scale}x");
            let image = decode(&bytes, target).unwrap();
            let size = image.size(0);
            assert_eq!(
                (size.width.0, size.height.0),
                (target.w as i32, target.h as i32),
                "drawn at about its own size at {scale}x"
            );
        }
    }
}
