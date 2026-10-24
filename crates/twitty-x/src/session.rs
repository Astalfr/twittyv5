//! Session credentials: the `auth_token` and `ct0` cookies of a logged-in x.com session.

use std::{fmt, path::Path};

use serde::{Deserialize, Serialize};

/// A secret string. Never printed: `Debug` and `Display` are redacted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Only for building request headers.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The two cookies X needs for authenticated GraphQL calls.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Session cookie.
    pub auth_token: Secret,
    /// CSRF cookie; also sent as the `x-csrf-token` header.
    pub ct0: Secret,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session { auth_token: ***, ct0: *** }")
    }
}

/// Why a session could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("environment variables TWITTY_AUTH_TOKEN and TWITTY_CT0 must both be set")]
    MissingEnv,
    #[error("could not read session file: {0}")]
    Io(#[from] std::io::Error),
    #[error("session file is not valid JSON with \"auth_token\" and \"ct0\": {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("session values must be non-empty and contain no ';', ',' or whitespace")]
    Malformed,
}

impl Session {
    /// Builds a session, rejecting values that would corrupt the `Cookie` header.
    pub fn new(
        auth_token: impl Into<String>,
        ct0: impl Into<String>,
    ) -> Result<Self, SessionError> {
        let (auth_token, ct0) = (auth_token.into(), ct0.into());
        let ok = |v: &str| {
            !v.is_empty() && !v.chars().any(|c| c == ';' || c == ',' || c.is_whitespace())
        };
        if !ok(&auth_token) || !ok(&ct0) {
            return Err(SessionError::Malformed);
        }
        Ok(Self { auth_token: Secret(auth_token), ct0: Secret(ct0) })
    }

    /// Reads `TWITTY_AUTH_TOKEN` and `TWITTY_CT0`.
    pub fn from_env() -> Result<Self, SessionError> {
        match (std::env::var("TWITTY_AUTH_TOKEN"), std::env::var("TWITTY_CT0")) {
            (Ok(a), Ok(c)) => Self::new(a.trim(), c.trim()),
            _ => Err(SessionError::MissingEnv),
        }
    }

    /// Reads `{"auth_token": "...", "ct0": "..."}` from a file.
    pub fn from_file(path: &Path) -> Result<Self, SessionError> {
        let raw: Session = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        Self::new(raw.auth_token.0.trim(), raw.ct0.0.trim())
    }

    /// Value of the `Cookie` request header.
    pub(crate) fn cookie_header(&self) -> String {
        format!("auth_token={}; ct0={}", self.auth_token.0, self.ct0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_values() {
        let s = Session::new("supersecrettoken", "csrfvalue123").unwrap();
        let shown = format!("{s:?} {:?} {:?}", s.auth_token, s.ct0);
        assert!(!shown.contains("supersecrettoken"));
        assert!(!shown.contains("csrfvalue123"));
    }

    #[test]
    fn rejects_values_that_break_cookie_header() {
        assert!(Session::new("", "x").is_err());
        assert!(Session::new("a;b", "x").is_err());
        assert!(Session::new("a b", "x").is_err());
        assert!(Session::new("a", "x,y").is_err());
    }

    #[test]
    fn cookie_header_format() {
        let s = Session::new("tok", "csrf").unwrap();
        assert_eq!(s.cookie_header(), "auth_token=tok; ct0=csrf");
    }

    #[test]
    fn reads_json_file_and_trims() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, r#"{"auth_token":" tok \n","ct0":"csrf"}"#).unwrap();
        assert_eq!(Session::from_file(&path).unwrap(), Session::new("tok", "csrf").unwrap());
    }

    #[test]
    fn bad_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, r#"{"auth_token":"tok"}"#).unwrap();
        assert!(matches!(Session::from_file(&path), Err(SessionError::Invalid(_))));
    }
}
