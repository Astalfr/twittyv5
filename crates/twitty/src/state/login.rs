//! The sign-in dialog's state, and when the sign-in window has a session.
//!
//! Two ways in: signing in on x.com in a window (`browser_login`), or pasting
//! the cookies. Only normalisation happens here; whether the values are valid
//! cookies is decided by the connector, and whether they work by the backend.

use std::fmt;

use twitty_model::BackendError;

use crate::{
    account::{ConnectError, Credentials},
    browser_login::BrowserError,
};

/// Where the sign-in attempt is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStatus {
    Editing,
    /// The x.com sign-in window is open.
    Browsing,
    Checking,
    Failed(String),
}

/// Where the credentials being checked came from, which decides the advice on failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Browser,
    Paste,
}

/// State of the sign-in dialog. `Debug` never shows the pasted values.
#[derive(Clone, PartialEq, Eq)]
pub struct LoginForm {
    status: LoginStatus,
    method: Method,
    /// The cookie fields (and how to find the cookies) are shown.
    paste_shown: bool,
    /// Shown above the form, e.g. "Your session expired".
    notice: Option<String>,
}

impl fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginForm")
            .field("status", &self.status)
            .field("method", &self.method)
            .field("paste_shown", &self.paste_shown)
            .field("notice", &self.notice)
            .finish()
    }
}

impl LoginForm {
    /// Without a sign-in window, the cookie fields are the only way in and start shown.
    pub fn new(notice: Option<String>, has_browser: bool) -> Self {
        Self {
            status: LoginStatus::Editing,
            method: Method::Paste,
            paste_shown: !has_browser,
            notice,
        }
    }

    pub fn paste_shown(&self) -> bool {
        self.paste_shown
    }

    pub fn toggle_paste(&mut self) {
        self.paste_shown = !self.paste_shown;
    }

    /// Whether a window or a check is running (the form is locked meanwhile).
    pub fn busy(&self) -> bool {
        matches!(self.status, LoginStatus::Browsing | LoginStatus::Checking)
    }

    /// Opens the sign-in window. `false` when something is already running.
    pub fn start_browser(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        self.status = LoginStatus::Browsing;
        true
    }

    /// The sign-in window closed. Returns the credentials to check, if any.
    pub fn browser_finished(
        &mut self,
        result: Result<Option<Credentials>, BrowserError>,
    ) -> Option<Credentials> {
        match result {
            Ok(Some(credentials)) => {
                self.status = LoginStatus::Checking;
                self.method = Method::Browser;
                Some(credentials)
            }
            // Closed by the user, or cancelled: nothing to report.
            Ok(None) => {
                self.status = LoginStatus::Editing;
                None
            }
            Err(e) => {
                self.status = LoginStatus::Failed(format!(
                    "{e}. You can paste the cookies from your browser instead."
                ));
                self.paste_shown = true;
                None
            }
        }
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
        if self.busy() {
            return None;
        }
        let auth_token = normalise(auth_token, "auth_token");
        let ct0 = normalise(ct0, "ct0");
        if auth_token.is_empty() || ct0.is_empty() {
            self.status = LoginStatus::Failed("Paste both auth_token and ct0.".into());
            return None;
        }
        self.status = LoginStatus::Checking;
        self.method = Method::Paste;
        Some(Credentials { auth_token, ct0 })
    }

    /// The connector refused the values.
    pub fn connect_failed(&mut self, error: &ConnectError) {
        self.status = LoginStatus::Failed(error.to_string());
    }

    /// The backend refused the session.
    pub fn check_failed(&mut self, error: &BackendError) {
        let message = match (error, self.method) {
            (BackendError::Unauthorized, Method::Browser) => {
                "Twitter didn't accept the session. Try signing in again.".to_owned()
            }
            (BackendError::Unauthorized, Method::Paste) => {
                "Twitter didn't accept these cookies. Copy them again from a logged-in x.com tab."
                    .to_owned()
            }
            (other, _) => format!("Couldn't check the session: {other}"),
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

/// Polls of an unchanged session before the sign-in window reports it.
const STABLE_POLLS: u32 = 2;

/// Decides when the cookies seen in the sign-in window are the signed-in
/// session. x.com sets a guest `ct0` before signing in and replaces it when
/// `auth_token` arrives, so a pair is only taken once it stops changing.
/// `Debug` never shows the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CookieWatch {
    last: Option<Credentials>,
    /// Polls since `last` was first seen.
    unchanged: u32,
}

impl fmt::Debug for CookieWatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieWatch")
            .field("seen", &self.last.is_some())
            .field("unchanged", &self.unchanged)
            .finish()
    }
}

impl CookieWatch {
    /// Takes one poll of the x.com cookies. Returns the session once it is settled.
    pub fn observe(&mut self, auth_token: Option<&str>, ct0: Option<&str>) -> Option<Credentials> {
        fn present(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|v| !v.is_empty())
        }
        let (Some(auth_token), Some(ct0)) = (present(auth_token), present(ct0)) else {
            *self = Self::default();
            return None;
        };
        let seen = Credentials { auth_token: auth_token.to_owned(), ct0: ct0.to_owned() };
        if self.last.as_ref() == Some(&seen) {
            self.unchanged += 1;
        } else {
            self.last = Some(seen);
            self.unchanged = 0;
        }
        if self.unchanged >= STABLE_POLLS { self.last.clone() } else { None }
    }
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
        let mut form = LoginForm::new(None, false);
        assert_eq!(form.submit(" ", "x"), None);
        assert!(matches!(form.status(), LoginStatus::Failed(_)));
        let creds = form.submit("auth_token=a", "c").unwrap();
        assert_eq!((creds.auth_token.as_str(), creds.ct0.as_str()), ("a", "c"));
        assert_eq!(form.status(), &LoginStatus::Checking);
        assert_eq!(form.submit("a", "c"), None, "one check at a time");
    }

    #[test]
    fn failures_explain_what_to_do() {
        let mut form = LoginForm::new(None, false);
        form.submit("a", "c");
        form.check_failed(&BackendError::Unauthorized);
        let LoginStatus::Failed(message) = form.status() else { panic!("failed") };
        assert!(message.contains("didn't accept"));
        assert!(form.submit("a", "c").is_some(), "can retry after a failure");
    }

    #[test]
    fn debug_shows_no_values() {
        let mut form = LoginForm::new(None, false);
        form.submit("secret-token", "secret-ct0");
        assert!(!format!("{form:?}").contains("secret"));
    }

    fn creds(a: &str, c: &str) -> Credentials {
        Credentials { auth_token: a.into(), ct0: c.into() }
    }

    #[test]
    fn paste_fields_start_hidden_only_with_a_window() {
        assert!(!LoginForm::new(None, true).paste_shown());
        assert!(LoginForm::new(None, false).paste_shown());
        let mut form = LoginForm::new(None, true);
        form.toggle_paste();
        assert!(form.paste_shown());
    }

    #[test]
    fn browser_session_is_checked() {
        let mut form = LoginForm::new(None, true);
        assert!(form.start_browser());
        assert_eq!(form.status(), &LoginStatus::Browsing);
        assert!(!form.start_browser(), "one window at a time");
        assert_eq!(form.submit("a", "c"), None, "no paste while the window is open");
        assert_eq!(form.browser_finished(Ok(Some(creds("a", "c")))), Some(creds("a", "c")));
        assert_eq!(form.status(), &LoginStatus::Checking);
        form.check_failed(&BackendError::Unauthorized);
        let LoginStatus::Failed(message) = form.status() else { panic!("failed") };
        assert!(message.contains("signing in again"), "{message}");
    }

    #[test]
    fn closing_the_window_is_not_an_error() {
        let mut form = LoginForm::new(None, true);
        form.start_browser();
        assert_eq!(form.browser_finished(Ok(None)), None);
        assert_eq!(form.status(), &LoginStatus::Editing);
        assert!(!form.paste_shown());
    }

    #[test]
    fn a_broken_window_points_to_pasting() {
        let mut form = LoginForm::new(None, true);
        form.start_browser();
        assert_eq!(form.browser_finished(Err(BrowserError::Launch("gone".into()))), None);
        let LoginStatus::Failed(message) = form.status() else { panic!("failed") };
        assert!(message.contains("paste"), "{message}");
        assert!(form.paste_shown());
    }

    #[test]
    fn watch_waits_for_auth_token() {
        let mut watch = CookieWatch::default();
        for _ in 0..5 {
            assert_eq!(watch.observe(None, Some("guest")), None);
            assert_eq!(watch.observe(Some(" "), Some("guest")), None);
        }
    }

    #[test]
    fn watch_takes_the_settled_ct0() {
        let mut watch = CookieWatch::default();
        assert_eq!(watch.observe(None, Some("guest")), None);
        assert_eq!(watch.observe(Some("tok"), Some("guest")), None);
        assert_eq!(watch.observe(Some("tok"), Some("fresh")), None, "ct0 just changed");
        assert_eq!(watch.observe(Some("tok"), Some("fresh")), None);
        assert_eq!(watch.observe(Some("tok"), Some("fresh")), Some(creds("tok", "fresh")));
    }

    #[test]
    fn watch_starts_over_when_the_session_goes() {
        let mut watch = CookieWatch::default();
        watch.observe(Some("tok"), Some("c"));
        watch.observe(Some("tok"), Some("c"));
        assert_eq!(watch.observe(None, None), None);
        assert_eq!(watch.observe(Some("tok"), Some("c")), None);
    }

    #[test]
    fn watch_debug_shows_no_values() {
        let mut watch = CookieWatch::default();
        watch.observe(Some("secret-token"), Some("secret-ct0"));
        assert!(!format!("{watch:?}").contains("secret"));
    }
}
