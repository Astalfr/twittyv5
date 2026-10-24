//! Buttons with a label: GPUI Kit's, a little roomier and bolder. Icon-only
//! buttons keep GPUI Kit's square sizes.

use gpui_kit::{
    FontWeight, Pixels, Styled,
    component::{Sizable, Size, button::Button},
    px,
};

/// Label weight of every text button.
const WEIGHT: FontWeight = FontWeight::SEMIBOLD;

/// (height, horizontal padding) of a text button of `size`. GPUI Kit's are
/// 20/24/32/32px tall with 4/8/10/12px each side; X's buttons are roomier.
pub(crate) fn metrics(size: Size) -> (Pixels, Pixels) {
    match size {
        Size::XSmall => (px(22.), px(8.)),
        Size::Small => (px(28.), px(12.)),
        Size::Large => (px(36.), px(18.)),
        Size::Medium | Size::Size(_) => (px(36.), px(16.)),
    }
}

/// Twitty's look for buttons with a label.
pub(crate) trait TextButton {
    /// Sizes the button to `size` with Twitty's padding and label weight.
    fn text_button(self, size: Size) -> Self;
}

impl TextButton for Button {
    fn text_button(self, size: Size) -> Self {
        let (height, padding) = metrics(size);
        self.with_size(size).h(height).px(padding).font_weight(WEIGHT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_buttons_are_roomier_than_gpui_kits() {
        // GPUI Kit's own (height, padding) for text buttons.
        let kit = [
            (Size::XSmall, px(20.), px(4.)),
            (Size::Small, px(24.), px(8.)),
            (Size::Medium, px(32.), px(10.)),
            (Size::Large, px(32.), px(12.)),
        ];
        for (size, height, padding) in kit {
            let (h, p) = metrics(size);
            assert!(h >= height, "{size:?} height {h:?}");
            assert!(p > padding, "{size:?} padding {p:?}");
        }
    }

    #[test]
    fn larger_sizes_never_get_less_room() {
        let sizes = [Size::XSmall, Size::Small, Size::Medium, Size::Large].map(metrics);
        for pair in sizes.windows(2) {
            assert!(pair[0].0 <= pair[1].0 && pair[0].1 <= pair[1].1, "{pair:?}");
        }
    }
}
