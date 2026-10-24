//! The retweet dialog: what confirming it does for a tweet.

use crate::state::interactions::Effective;

/// What the dialog's main button does, from the viewer's current state of
/// the tweet (with their pending toggles, not the state the tweet was loaded
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
            Self::Repost => "Retweet this tweet?",
            Self::Undo => "Undo this retweet?",
        }
    }

    /// The main button's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Repost => "Retweet",
            Self::Undo => "Undo retweet",
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
        assert!(!post.viewer.reposted, "the loaded tweet is unchanged");
        assert_eq!(RepostChoice::for_post(&state.effective(&post)), RepostChoice::Undo);
        assert_eq!(RepostChoice::Undo.label(), "Undo retweet");
    }
}
