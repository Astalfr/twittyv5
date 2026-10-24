//! Request headers X's web client sends.

use std::path::Path;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

/// The public bearer token embedded in x.com's JavaScript. Identical for every
/// web user; not a credential. Re-extract it if every request gets an empty 403.
pub const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

/// A current desktop Chrome user agent.
pub const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Stable per-install identifiers the web client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIds {
    pub client_uuid: String,
    pub device_id: String,
}

impl ClientIds {
    /// Fresh random identifiers.
    pub fn random() -> Self {
        Self {
            client_uuid: uuid::Uuid::new_v4().to_string(),
            device_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Loads identifiers from `path`, creating and saving new ones if missing or invalid.
    pub fn load_or_create(path: &Path) -> Self {
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(ids) = serde_json::from_str::<ClientIds>(&raw)
        {
            return ids;
        }
        let ids = Self::random();
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, serde_json::to_string(&ids).unwrap_or_default()));
        if let Err(e) = saved {
            tracing::warn!("could not save client ids to {}: {e}", path.display());
        }
        ids
    }
}

/// Headers sent with every API request. Credentials are added per request.
pub(crate) fn static_headers(ids: &ClientIds) -> HeaderMap {
    let mut map = HeaderMap::new();
    let mut put = |name: &'static str, value: &str| match HeaderValue::from_str(value) {
        Ok(v) => {
            map.insert(HeaderName::from_static(name), v);
        }
        Err(_) => tracing::error!("invalid value for header {name}"),
    };
    put("authorization", &format!("Bearer {WEB_BEARER}"));
    put("x-twitter-auth-type", "OAuth2Session");
    put("x-twitter-active-user", "yes");
    put("x-twitter-client-language", "en");
    put("accept", "*/*");
    put("accept-language", "en-US,en;q=0.9");
    put("origin", "https://x.com");
    put("referer", "https://x.com/");
    put("user-agent", CHROME_UA);
    put("x-client-uuid", &ids.client_uuid);
    put("x-twitter-client-deviceid", &ids.device_id);
    map
}

/// A value for `x-client-transaction-id`.
///
/// X currently accepts an opaque random value. If it starts answering with
/// error 353, the real algorithm must be implemented here (see docs/x-api.md).
pub(crate) fn transaction_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_headers_have_bearer_and_ids_but_no_credentials() {
        let ids = ClientIds::random();
        let h = static_headers(&ids);
        assert!(h["authorization"].to_str().unwrap().contains(WEB_BEARER));
        assert_eq!(h["x-twitter-auth-type"], "OAuth2Session");
        assert_eq!(h["x-client-uuid"].to_str().unwrap(), ids.client_uuid);
        assert!(h.get("cookie").is_none());
        assert!(h.get("x-csrf-token").is_none());
    }

    #[test]
    fn transaction_ids_are_unique_hex() {
        let a = transaction_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, transaction_id());
    }

    #[test]
    fn client_ids_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("ids.json");
        let first = ClientIds::load_or_create(&path);
        assert_eq!(ClientIds::load_or_create(&path), first);
    }
}
