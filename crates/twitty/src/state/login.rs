//! The sign-in form: what the user pasted, and what the dialog shows.
//!
//! Only normalisation happens here; whether the values are valid cookies is
//! decided by the connector, and whether they work by the backend.

use std::fmt;

use twitty_model::BackendError;

use crate::account::{ConnectError, Credentials};

/// Where the sign-in attempt is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStatus {
    Editing,
    Checking,
    Failed(String),
}

/// State of the sign-in dialog. `Debug` never shows the pasted values.
#[derive(Clone, PartialEq, Eq)]
pub struct LoginForm {
    status: LoginStatus,
    /// Shown above the form, e.g. "Your session expired".
    notice: Option<String>,
}

impl fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginForm")
            .field("status", &self.status)
            .field("notice", &self.notice)
            .finish()
    }
}

impl LoginForm {
    pub fn new(notice: Option<String>) -> Self {
        Self { status: LoginStatus::Editing, notice }
    }

    pub fn status(&self) -> &LoginStatus {
        &self.status
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Starts checking the pasted values. `None` (with an error shown) when a
    /// field is empty or a check is already running.
    pub fn submit(&mut self, auth_token: &str, ct0: &str) -> Option<Credentials> {
        if self.status == LoginStatus::Checking {
            return None;
        }
        let auth_token = normalise(auth_token, "auth_token");
        let ct0 = normalise(ct0, "ct0");
        if auth_token.is_empty() || ct0.is_empty() {
            self.status = LoginStatus::Failed("Paste both auth_token and ct0.".into());
            return None;
        }
        self.status = LoginStatus::Checking;
        Some(Credentials { auth_token, ct0 })
    }

    /// The connector refused the values.
    pub fn connect_failed(&mut self, error: &ConnectError) {
        self.status = LoginStatus::Failed(error.to_string());
    }

    /// The backend refused the session.
    pub fn check_failed(&mut self, error: &BackendError) {
        let message = match error {
            BackendError::Unauthorized => {
                "X didn't accept these cookies. Copy them again from a logged-in x.com tab."
                    .to_owned()
            }
            other => format!("Couldn't check the session: {other}"),
        };
        self.status = LoginStatus::Failed(message);
    }
}

/// Trims whitespace and quotes and strips a leading `name=`, so pasting a
/// DevTools row or a cookie-header fragment works.
pub fn normalise(value: &str, name: &str) -> String {
    let v = value.trim().trim_end_matches(';').trim();
    let v = v.strip_prefix(name).and_then(|rest| rest.trim_start().strip_prefix('=')).unwrap_or(v);
    v.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn normalise_accepts_common_paste_forms() {
        for pasted in
            ["abc123", "  abc123\n", "\"abc123\"", "auth_token=abc123;", "auth_token = abc123"]
        {
            assert_eq!(normalise(pasted, "auth_token"), "abc123", "{pasted:?}");
        }
        assert_eq!(normalise("ct0=abc", "auth_token"), "ct0=abc", "other names are left alone");
    }

    #[test]
    fn submit_requires_both_values() {
        let mut form = LoginForm::new(None);
        assert_eq!(form.submit(" ", "x"), None);
        assert!(matches!(form.status(), LoginStatus::Failed(_)));
        let creds = form.submit("auth_token=a", "c").unwrap();
        assert_eq!((creds.auth_token.as_str(), creds.ct0.as_str()), ("a", "c"));
        assert_eq!(form.status(), &LoginStatus::Checking);
        assert_eq!(form.submit("a", "c"), None, "one check at a time");
    }

    #[test]
    fn failures_explain_what_to_do() {
        let mut form = LoginForm::new(None);
        form.submit("a", "c");
        form.check_failed(&BackendError::Unauthorized);
        let LoginStatus::Failed(message) = form.status() else { panic!("failed") };
        assert!(message.contains("didn't accept"));
        assert!(form.submit("a", "c").is_some(), "can retry after a failure");
    }

    #[test]
    fn debug_shows_no_values() {
        let mut form = LoginForm::new(None);
        form.submit("secret-token", "secret-ct0");
        assert!(!format!("{form:?}").contains("secret"));
    }
}
