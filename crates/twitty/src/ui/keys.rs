//! Keyboard shortcuts. Deck shortcuts live in the `Deck` key context, which
//! dialogs are outside of, so typing in a text field never triggers them.

use gpui_kit::{App, KeyBinding, actions};

actions!(
    twitty,
    [
        /// Select the next post in the active column.
        SelectNext,
        /// Select the previous post in the active column.
        SelectPrevious,
        /// Make the column on the left active (or show the previous image).
        ColumnLeft,
        /// Make the column on the right active (or show the next image).
        ColumnRight,
        /// Like / unlike the selected post.
        LikeSelected,
        /// Ask whether to repost (or undo the repost of) the selected post,
        /// or quote it.
        RepostSelected,
        /// Bookmark / remove the bookmark of the selected post.
        BookmarkSelected,
        /// Reply to the selected post.
        ReplySelected,
        /// Open the selected post's conversation.
        OpenSelected,
        /// Write a new post.
        NewPost,
        /// Close the image viewer, dialog or thread panel.
        Dismiss,
        /// Repost (or undo the repost) from the repost dialog.
        ConfirmRepost,
        /// Quote the post instead, from the repost dialog.
        QuoteInstead,
    ]
);

/// Key context of the deck (rail, columns, thread panel).
pub const DECK: &str = "Deck";
/// Key context of a dialog overlay.
pub const OVERLAY: &str = "Overlay";
/// Key context of the repost dialog (inside the overlay).
pub const REPOST_DIALOG: &str = "RepostDialog";

/// Every shortcut, as shown in the settings page: (keys, what they do).
pub const HELP: [(&str, &str); 11] = [
    ("j / k", "Next / previous post"),
    ("← / →", "Previous / next column (or image)"),
    ("Enter", "Open the conversation"),
    ("l", "Like"),
    ("t", "Repost or quote (asks first)"),
    ("b", "Bookmark"),
    ("r", "Reply"),
    ("n", "New post"),
    ("Esc", "Close image, dialog or panel"),
    ("Enter (in a dialog)", "Submit"),
    ("q (in the repost dialog)", "Quote"),
];

/// Registers the shortcuts.
pub fn bind(cx: &mut App) {
    let deck = Some(DECK);
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, deck),
        KeyBinding::new("down", SelectNext, deck),
        KeyBinding::new("k", SelectPrevious, deck),
        KeyBinding::new("up", SelectPrevious, deck),
        KeyBinding::new("left", ColumnLeft, deck),
        KeyBinding::new("right", ColumnRight, deck),
        KeyBinding::new("l", LikeSelected, deck),
        KeyBinding::new("t", RepostSelected, deck),
        KeyBinding::new("b", BookmarkSelected, deck),
        KeyBinding::new("r", ReplySelected, deck),
        KeyBinding::new("enter", OpenSelected, deck),
        KeyBinding::new("n", NewPost, deck),
        KeyBinding::new("escape", Dismiss, deck),
        KeyBinding::new("escape", Dismiss, Some(OVERLAY)),
        KeyBinding::new("enter", ConfirmRepost, Some(REPOST_DIALOG)),
        KeyBinding::new("q", QuoteInstead, Some(REPOST_DIALOG)),
    ]);
}
