use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use url::Url;

use crate::{ImageRef, RichText, UserId};

/// A user account as displayed by Twitty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: UserId,
    /// Handle without the leading `@`.
    pub handle: SmolStr,
    pub display_name: String,
    pub avatar: Option<ImageRef>,
    pub banner: Option<ImageRef>,
    pub bio: RichText,
    pub location: Option<String>,
    pub website: Option<Url>,
    pub badges: Badges,
    pub counts: ProfileCounts,
    pub viewer: ViewerRelation,
}

impl Profile {
    /// Minimal profile with only identity fields set.
    pub fn new(id: UserId, handle: impl Into<SmolStr>, display_name: impl Into<String>) -> Self {
        Self {
            id,
            handle: handle.into(),
            display_name: display_name.into(),
            avatar: None,
            banner: None,
            bio: RichText::default(),
            location: None,
            website: None,
            badges: Badges::default(),
            counts: ProfileCounts::default(),
            viewer: ViewerRelation::default(),
        }
    }
}

/// Visual markers shown next to a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Badges {
    pub verified: Option<VerifiedKind>,
    /// Tweets are only visible to approved followers.
    pub protected: bool,
}

/// Which verification checkmark to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedKind {
    /// Paid / individual (blue).
    Individual,
    /// Organisation (gold).
    Business,
    /// Government or multilateral (gray).
    Government,
}

/// Public counters of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProfileCounts {
    pub followers: u64,
    pub following: u64,
    pub posts: u64,
}

/// How the signed-in viewer relates to a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewerRelation {
    pub following: bool,
    pub followed_by: bool,
    pub blocking: bool,
    pub muting: bool,
}
