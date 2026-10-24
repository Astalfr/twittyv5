use chrono::{DateTime, Utc};

/// Failure of a [`crate::Backend`] call, in terms the UI can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The session is missing, expired or revoked; the user must sign in again.
    #[error("not signed in or session expired")]
    Unauthorized,
    /// The provider's rate limit is exhausted until `reset_at`.
    #[error("rate limited until {reset_at}")]
    RateLimited { reset_at: DateTime<Utc> },
    /// The requested object does not exist (or is hidden from the viewer).
    #[error("not found")]
    NotFound,
    /// The provider rejected the request.
    #[error("provider error: {0}")]
    Upstream(String),
    /// Connection, TLS or timeout failure.
    #[error("network error: {0}")]
    Network(String),
    /// The response could not be understood at all.
    #[error("could not decode response: {0}")]
    Decode(String),
}

impl BackendError {
    /// Whether retrying later may succeed without user action.
    pub fn is_transient(&self) -> bool {
        matches!(self, BackendError::RateLimited { .. } | BackendError::Network(_))
    }
}
