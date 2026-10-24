//! Keyboard shortcuts. Deck shortcuts live in the `Deck` key context, which
//! dialogs are outside of, so typing in a text field never triggers them.

use gpui_kit::{App, KeyBinding, actions};

actions!(
    twitty,
    [
        /// Select the next tweet in the active column.
        SelectNext,
        /// Select the previous tweet in the active column.
        SelectPrevious,
        /// Make the column on the left active (or show the previous image).
        ColumnLeft,
        /// Make the column on the right active (or show the next image).
        ColumnRight,
        /// Like / unlike the selected tweet.
        LikeSelected,
        /// Ask whether to retweet (or undo the retweet of) the selected tweet,
        /// or quote it.
        RepostSelected,
        /// Bookmark / remove the bookmark of the selected tweet.
        BookmarkSelected,
        /// Reply to the selected tweet.
        ReplySelected,
        /// Open the selected tweet's conversation.
        OpenSelected,
        /// Write a new tweet.
        NewPost,
        /// Close the image viewer, dialog or thread panel.
        Dismiss,
        /// Retweet (or undo the retweet) from the retweet dialog.
        ConfirmRepost,
        /// Quote the tweet instead, from the retweet dialog.
        QuoteInstead,
    ]
);

/// Key context of the deck (rail, columns, thread panel).
pub const DECK: &str = "Deck";
/// Key context of a dialog overlay.
pub const OVERLAY: &str = "Overlay";
/// Key context of the retweet dialog (inside the overlay).
pub const REPOST_DIALOG: &str = "RepostDialog";

/// Every shortcut, as shown in the settings page: (keys, what they do).
pub const HELP: [(&str, &str); 11] = [
    ("j / k", "Next / previous tweet"),
    ("← / →", "Previous / next column (or image)"),
    ("Enter", "Open the conversation"),
    ("l", "Like"),
    ("t", "Retweet or quote (asks first)"),
    ("b", "Bookmark"),
    ("r", "Reply"),
    ("n", "New tweet"),
    ("Esc", "Close image, dialog or panel"),
    ("Enter (in a dialog)", "Submit"),
    ("q (in the retweet dialog)", "Quote"),
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
