use chrono::{DateTime, Utc};
use twitty_model::BackendError;

/// Failure of a call to Twitter.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum XError {
    /// Missing, expired or revoked session (HTTP 401/403, Twitter codes 32, 89, 215).
    #[error("not authorized (HTTP {status}{})", code.map(|c| format!(", X code {c}")).unwrap_or_default())]
    Unauthorized { status: u16, code: Option<i64> },
    /// Rate limit exhausted for this operation.
    #[error("rate limited on {operation} until {reset_at}")]
    RateLimited { operation: String, reset_at: DateTime<Utc> },
    /// Twitter requires a real `x-client-transaction-id` (error code 353).
    #[error(
        "X rejected the client transaction id (code 353); a real transaction id is now required"
    )]
    TransactionIdRequired,
    /// The operation's query id is unknown to Twitter, even after refreshing ids.
    #[error("query id {query_id} for {operation} is stale (HTTP 404) and no newer id was found")]
    StaleQueryId { operation: String, query_id: String },
    /// No query id is known for the operation.
    #[error("no query id known for operation {0}")]
    UnknownOperation(String),
    /// Twitter returned errors and no data.
    #[error("Twitter error {code}: {message}")]
    Api { code: i64, message: String },
    /// Unexpected HTTP status.
    #[error("unexpected HTTP {status}: {snippet}")]
    Http { status: u16, snippet: String },
    /// Connection, TLS or timeout failure.
    #[error("network error: {0}")]
    Network(String),
    /// The body was not the JSON we expected.
    #[error("could not decode response: {0}")]
    Decode(String),
    /// Query-id discovery failed.
    #[error("query id discovery failed: {0}")]
    Discovery(String),
}

impl From<reqwest::Error> for XError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors can contain the URL but never headers, so no secrets leak here.
        XError::Network(e.to_string())
    }
}

impl From<XError> for BackendError {
    fn from(e: XError) -> Self {
        match e {
            XError::Unauthorized { .. } => BackendError::Unauthorized,
            XError::RateLimited { reset_at, .. } => BackendError::RateLimited { reset_at },
            XError::Network(m) => BackendError::Network(m),
            XError::Decode(m) => BackendError::Decode(m),
            XError::Api { code: 34 | 144, .. } => BackendError::NotFound,
            other => BackendError::Upstream(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_backend_errors() {
        assert_eq!(
            BackendError::from(XError::Unauthorized { status: 401, code: None }),
            BackendError::Unauthorized
        );
        assert_eq!(
            BackendError::from(XError::Api { code: 144, message: "gone".into() }),
            BackendError::NotFound
        );
        assert!(matches!(
            BackendError::from(XError::TransactionIdRequired),
            BackendError::Upstream(_)
        ));
    }

    #[test]
    fn unauthorized_message_includes_code() {
        let e = XError::Unauthorized { status: 403, code: Some(32) };
        assert_eq!(e.to_string(), "not authorized (HTTP 403, X code 32)");
    }
}
