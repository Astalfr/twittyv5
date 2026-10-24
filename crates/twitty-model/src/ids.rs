use std::fmt;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(SmolStr);

        impl $name {
            /// Wraps a backend-provided identifier. The value is opaque to the app.
            pub fn new(id: impl AsRef<str>) -> Self {
                Self(SmolStr::new(id.as_ref()))
            }

            /// The raw identifier, for backends and logging only.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a tweet. Opaque outside the backend that issued it.
    PostId
);
id_type!(
    /// Identifies a user account.
    UserId
);
id_type!(
    /// Identifies a list.
    ListId
);
id_type!(
    /// Identifies a notification.
    NotificationId
);

/// An opaque paging cursor. Only the backend that produced it can interpret
/// it, and it is never persisted beyond a session.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PageToken(String);

impl PageToken {
    /// Wraps a backend cursor.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The raw cursor, for the issuing backend only.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PageToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Cursors are long and meaningless in logs; show only a prefix.
        let prefix: String = self.0.chars().take(12).collect();
        write!(f, "PageToken({prefix}…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_id_serializes_as_plain_string() {
        let id = PostId::new("123");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"123\"");
        let back: PostId = serde_json::from_str("\"123\"").unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn page_token_debug_is_truncated() {
        let token = PageToken::new("DAABCgABGlong-cursor-value-here");
        assert_eq!(format!("{token:?}"), "PageToken(DAABCgABGlon…)");
    }
}
