//! The repost dialog: what confirming it does for a post.

use crate::state::interactions::Effective;

/// What the dialog's main button does, from the viewer's current state of
/// the post (with their pending toggles, not the state the post was loaded
/// with).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepostChoice {
    Repost,
    Undo,
}

impl RepostChoice {
    pub fn for_post(eff: &Effective) -> Self {
        if eff.viewer.reposted { Self::Undo } else { Self::Repost }
    }

    /// The dialog's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Repost => "Repost this post?",
            Self::Undo => "Undo this repost?",
        }
    }

    /// The main button's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Repost => "Repost",
            Self::Undo => "Undo repost",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::interactions::{Interactions, Toggle};
    use twitty_model::{Backend, Draft, mock::MockBackend};

    #[test]
    fn follows_the_pending_toggle_not_the_loaded_post() {
        let backend = MockBackend::new();
        let post = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(backend.publish(Draft { text: "hi".into(), ..Default::default() }))
            .unwrap();
        let mut state = Interactions::default();
        assert_eq!(RepostChoice::for_post(&state.effective(&post)), RepostChoice::Repost);
        state.toggle(&post, Toggle::Repost);
        assert!(!post.viewer.reposted, "the loaded post is unchanged");
        assert_eq!(RepostChoice::for_post(&state.effective(&post)), RepostChoice::Undo);
        assert_eq!(RepostChoice::Undo.label(), "Undo repost");
    }
}
