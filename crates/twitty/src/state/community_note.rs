//! Community Notes: where a post's note is shown in full, and where only a
//! compact alert says it has one.
//!
//! The note is read in the thread panel, under the post the panel is about.
//! Everywhere else (columns, profiles, the panel's other posts) a compact
//! alert under the post's text, "Readers added context", says there is one;
//! clicking it, like the rest of the row, opens the panel.

use twitty_model::{CommunityNote, Post, PostId};

/// The heading of a note shown in full, as on x.com.
pub const TITLE: &str = "Readers added context they thought people might want to know";
/// The text of the compact alert under a post with a note.
pub const MARKER: &str = "Readers added context";
/// The link to the note's page, where it can be rated.
pub const RATE: &str = "Rate it on X";

/// How a post's note is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteDisplay<'a> {
    /// The post has no note.
    None,
    /// A compact alert under the post's text.
    Marker,
    /// The whole note under the post.
    Full(&'a CommunityNote),
}

/// How to show `post`'s note, when `focus` is the post a thread panel is
/// about (`None` outside the panel).
pub fn display<'a>(post: &'a Post, focus: Option<&PostId>) -> NoteDisplay<'a> {
    match &post.community_note {
        None => NoteDisplay::None,
        Some(note) if focus == Some(&post.id) => NoteDisplay::Full(note),
        Some(_) => NoteDisplay::Marker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twitty_model::{Backend, FeedItem, FeedSource, HomeMode, PageRequest, mock::MockBackend};

    fn posts() -> Vec<Post> {
        let backend = MockBackend::new();
        let page = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                backend.feed(&FeedSource::Home { mode: HomeMode::Following }, PageRequest::First),
            )
            .unwrap();
        page.items.iter().flat_map(FeedItem::posts).cloned().collect()
    }

    fn noted() -> Post {
        posts().into_iter().find(|p| p.community_note.is_some()).expect("the mock has a noted post")
    }

    #[test]
    fn the_focused_post_shows_its_note_in_full() {
        let post = noted();
        let NoteDisplay::Full(note) = display(&post, Some(&post.id)) else {
            panic!("full note expected")
        };
        assert_eq!(Some(note), post.community_note.as_ref());
    }

    #[test]
    fn elsewhere_a_noted_post_only_shows_the_marker() {
        let post = noted();
        assert_eq!(display(&post, None), NoteDisplay::Marker);
        let other = posts().into_iter().find(|p| p.id != post.id).unwrap();
        assert_eq!(display(&post, Some(&other.id)), NoteDisplay::Marker, "another post's panel");
    }

    #[test]
    fn a_post_without_note_shows_nothing() {
        let mut post = noted();
        post.community_note = None;
        assert_eq!(display(&post, Some(&post.id)), NoteDisplay::None);
        assert_eq!(display(&post, None), NoteDisplay::None);
    }
}
