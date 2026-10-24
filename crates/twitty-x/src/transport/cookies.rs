//! Picking rotated `ct0` / `auth_token` values out of `Set-Cookie` headers.

use reqwest::header::{HeaderMap, SET_COOKIE};

/// New session cookie values announced by a response.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Rotated {
    pub auth_token: Option<String>,
    pub ct0: Option<String>,
}

impl Rotated {
    pub fn is_empty(&self) -> bool {
        self.auth_token.is_none() && self.ct0.is_none()
    }
}

/// Reads `Set-Cookie` headers. Empty values (cookie deletion) are ignored:
/// a logout is detected through 401s, not by wiping the stored session.
pub(crate) fn rotated(headers: &HeaderMap) -> Rotated {
    let mut out = Rotated::default();
    for value in headers.get_all(SET_COOKIE) {
        let Ok(value) = value.to_str() else { continue };
        let first = value.split(';').next().unwrap_or_default();
        let Some((name, val)) = first.split_once('=') else { continue };
        let val = val.trim().trim_matches('"');
        if val.is_empty() {
            continue;
        }
        match name.trim() {
            "auth_token" => out.auth_token = Some(val.to_owned()),
            "ct0" => out.ct0 = Some(val.to_owned()),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn with(cookies: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for c in cookies {
            h.append(SET_COOKIE, HeaderValue::from_str(c).unwrap());
        }
        h
    }

    #[test]
    fn picks_session_cookies_only() {
        let h = with(&[
            "guest_id=v1%3A1; Max-Age=34214400; Domain=.x.com; Path=/; Secure",
            "ct0=newcsrf; Max-Age=21600; Domain=.x.com; Path=/; Secure; SameSite=Lax",
            "auth_token=newtok; Domain=.x.com; Path=/; Secure; HttpOnly",
        ]);
        assert_eq!(
            rotated(&h),
            Rotated { auth_token: Some("newtok".into()), ct0: Some("newcsrf".into()) }
        );
    }

    #[test]
    fn deletions_and_garbage_are_ignored() {
        let h = with(&["ct0=; Max-Age=0; Path=/", "auth_token", "=x"]);
        assert!(rotated(&h).is_empty());
    }
}
