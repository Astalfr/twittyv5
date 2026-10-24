//! HTTP layer for X's internal API: authentication, cookie rotation,
//! rate limits, retries and error mapping. Knows nothing about GraphQL shapes.

mod cookies;
mod headers;
mod rate_limit;
mod transaction;

use std::{
    fmt,
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use chrono::Utc;
use reqwest::{StatusCode, header::HeaderMap};
use serde_json::Value;
use url::Url;

pub use headers::{CHROME_UA, ClientIds, WEB_BEARER};
pub use rate_limit::{RateLimit, RateLimits};

use crate::{XError, catalog::Method, session::Session};

/// Called with the new session after X rotated a cookie, so it can be persisted.
pub type RotateHook = Arc<dyn Fn(&Session) + Send + Sync>;

/// Retry behaviour for transient failures (network errors, 5xx, 429 without reset).
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Delay before the first retry; doubles each time.
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_retries: 3, base_delay: Duration::from_secs(1) }
    }
}

/// One API request.
#[derive(Debug, Clone)]
pub(crate) struct ApiRequest {
    /// Rate-limit bucket and log label (the GraphQL operation name, or the
    /// REST path).
    pub endpoint: String,
    pub method: Method,
    /// Path below the API base, e.g. `graphql/<id>/HomeTimeline` or
    /// `1.1/friendships/create.json`.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Body>,
}

/// The body of a POST.
#[derive(Debug, Clone)]
pub(crate) enum Body {
    /// GraphQL calls.
    Json(Value),
    /// The v1.1 REST endpoints (e.g. `friendships/create.json`), which take
    /// form fields.
    Form(Vec<(String, String)>),
}

/// Authenticated client for `https://x.com/i/api/`.
pub struct Transport {
    http: reqwest::Client,
    api_base: Url,
    session: RwLock<Session>,
    on_rotate: Option<RotateHook>,
    limits: RateLimits,
    retry: RetryPolicy,
    /// Signs requests with a real `x-client-transaction-id`; `None` sends
    /// random ids (tests, which must not fetch x.com).
    signer: Option<transaction::Signer>,
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("api_base", &self.api_base.as_str())
            .field("session", &"***")
            .field("retry", &self.retry)
            .field("signer", &self.signer)
            .finish_non_exhaustive()
    }
}

impl Transport {
    /// Production API base.
    pub const API_BASE: &str = "https://x.com/i/api/";
    /// The page whose key and animation sign requests.
    pub const SIGNING_PAGE: &str = "https://x.com/home";

    /// Builds a transport. `api_base` must end with `/`.
    pub fn new(
        session: Session,
        ids: &ClientIds,
        api_base: Url,
        retry: RetryPolicy,
    ) -> Result<Self, XError> {
        let http = reqwest::Client::builder()
            .default_headers(headers::static_headers(ids))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            api_base,
            session: RwLock::new(session),
            on_rotate: None,
            limits: RateLimits::default(),
            retry,
            signer: None,
        })
    }

    /// Signs requests with a real `x-client-transaction-id` derived from
    /// `page` (use [`Self::SIGNING_PAGE`]). The page is fetched, logged in,
    /// before the first request and again every hour; until it can be read,
    /// requests carry a random id.
    pub fn signing_from(mut self, page: Url) -> Result<Self, XError> {
        self.signer = Some(transaction::Signer::new(page)?);
        Ok(self)
    }

    /// Registers a callback for cookie rotation.
    pub fn on_rotate(mut self, hook: RotateHook) -> Self {
        self.on_rotate = Some(hook);
        self
    }

    /// The current session (after any rotation).
    pub fn session(&self) -> Session {
        self.session.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Per-operation rate-limit budgets.
    pub fn limits(&self) -> &RateLimits {
        &self.limits
    }

    /// Sends `req`, retrying transient failures, and returns the JSON body.
    pub(crate) async fn send(&self, req: &ApiRequest) -> Result<Value, XError> {
        if let Err(reset_at) = self.limits.check(&req.endpoint, Utc::now()) {
            return Err(XError::RateLimited { operation: req.endpoint.clone(), reset_at });
        }
        let url = self
            .api_base
            .join(&req.path)
            .map_err(|e| XError::Decode(format!("bad request path {}: {e}", req.path)))?;

        let mut attempt = 0;
        loop {
            let outcome = self.send_once(req, &url).await;
            let retryable = match &outcome {
                Err(XError::Network(_)) => true,
                Err(XError::Http { status, .. }) => *status >= 500,
                Err(XError::RateLimited { .. }) => false,
                _ => false,
            };
            if !retryable || attempt >= self.retry.max_retries {
                return outcome;
            }
            let delay = self.retry.base_delay * 2u32.saturating_pow(attempt);
            tracing::debug!(endpoint = %req.endpoint, attempt, ?delay, "retrying: {outcome:?}");
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    async fn send_once(&self, req: &ApiRequest, url: &Url) -> Result<Value, XError> {
        let session = self.session();
        let transaction_id = match &self.signer {
            Some(signer) => signer.id(&session, req.method, url.path()).await,
            None => transaction::random_id(),
        };
        let mut builder = match req.method {
            Method::Get => self.http.get(url.clone()),
            Method::Post => self.http.post(url.clone()),
        }
        .header("cookie", session.cookie_header())
        .header("x-csrf-token", session.ct0.expose())
        .header("x-client-transaction-id", transaction_id)
        .query(&req.query);
        match &req.body {
            Some(Body::Json(body)) => builder = builder.json(body),
            Some(Body::Form(fields)) => {
                let encoded = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields)
                    .finish();
                builder = builder
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(encoded);
            }
            None => {}
        }

        let response = builder.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        self.limits.observe(&req.endpoint, &headers);
        self.apply_rotation(&headers);
        let text = response.text().await?;
        interpret(&req.endpoint, status, &headers, &text)
    }

    fn apply_rotation(&self, headers: &HeaderMap) {
        let rotated = cookies::rotated(headers);
        if rotated.is_empty() {
            return;
        }
        let updated = {
            let mut session = self.session.write().unwrap_or_else(PoisonError::into_inner);
            let mut next = session.clone();
            if let Some(v) = rotated.auth_token {
                next.auth_token = crate::Secret::new(v);
            }
            if let Some(v) = rotated.ct0 {
                next.ct0 = crate::Secret::new(v);
            }
            if next == *session {
                return;
            }
            *session = next.clone();
            next
        };
        tracing::info!("X rotated session cookies");
        if let Some(hook) = &self.on_rotate {
            hook(&updated);
        }
    }
}

/// Codes X sends with a 403 when it refuses an action but the session is
/// fine: 158 "can't follow yourself", 160 "already requested to follow",
/// 161 "follow limit", 162 "blocked from following". Mapping them to
/// `Unauthorized` would ask the user to sign in again.
const REFUSED: [i64; 4] = [158, 160, 161, 162];

/// Maps an HTTP response to the JSON body or an error.
fn interpret(
    endpoint: &str,
    status: StatusCode,
    headers: &HeaderMap,
    text: &str,
) -> Result<Value, XError> {
    let json: Option<Value> = serde_json::from_str(text).ok();
    let first_code = json.as_ref().and_then(first_error_code);
    match status.as_u16() {
        200..=299 => {
            let body = json.ok_or_else(|| XError::Decode(snippet(text)))?;
            interpret_body(endpoint, body)
        }
        401 => Err(XError::Unauthorized { status: 401, code: first_code }),
        403 if first_code == Some(353) => Err(XError::TransactionIdRequired),
        403 if first_code.is_some_and(|c| REFUSED.contains(&c)) => {
            let message = json
                .as_ref()
                .and_then(|j| j.get("errors")?.get(0)?.get("message")?.as_str())
                .unwrap_or("refused")
                .to_owned();
            Err(XError::Api { code: first_code.unwrap_or(0), message })
        }
        403 => Err(XError::Unauthorized { status: 403, code: first_code }),
        429 => {
            let reset_at = RateLimit::from_headers(headers)
                .map(|l| l.reset_at)
                .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(1));
            Err(XError::RateLimited { operation: endpoint.to_owned(), reset_at })
        }
        status => Err(XError::Http { status, snippet: snippet(text) }),
    }
}

/// A 2xx body may still carry `errors`. With usable `data` we keep the data
/// (X often reports partial failures this way); without it we fail.
fn interpret_body(endpoint: &str, body: Value) -> Result<Value, XError> {
    let errors = body.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty());
    let has_data = body.get("data").is_some_and(|d| !d.is_null());
    match (errors, has_data) {
        (None, _) => Ok(body),
        (Some(errs), true) => {
            tracing::warn!(endpoint, count = errs.len(), "X returned data with partial errors");
            Ok(body)
        }
        (Some(errs), false) => {
            let first = &errs[0];
            let code = first.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message =
                first.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_owned();
            Err(match code {
                32 | 89 | 215 => XError::Unauthorized { status: 200, code: Some(code) },
                88 => XError::RateLimited {
                    operation: endpoint.to_owned(),
                    reset_at: Utc::now() + chrono::Duration::minutes(1),
                },
                353 => XError::TransactionIdRequired,
                _ => XError::Api { code, message },
            })
        }
    }
}

fn first_error_code(body: &Value) -> Option<i64> {
    body.get("errors")?.as_array()?.first()?.get("code")?.as_i64()
}

fn snippet(text: &str) -> String {
    let mut s: String = text.chars().take(200).collect();
    if text.chars().count() > 200 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn interp(status: u16, body: &str) -> Result<Value, XError> {
        interpret("Op", StatusCode::from_u16(status).unwrap(), &HeaderMap::new(), body)
    }

    #[test]
    fn ok_body_passes_through() {
        assert_eq!(interp(200, r#"{"data":{"x":1}}"#).unwrap(), json!({"data":{"x":1}}));
    }

    #[test]
    fn data_with_errors_is_kept() {
        let body = r#"{"data":{"x":1},"errors":[{"code":37,"message":"partial"}]}"#;
        assert!(interp(200, body).is_ok());
    }

    #[test]
    fn errors_without_data_are_mapped() {
        let e = |code: i64| {
            interp(200, &format!(r#"{{"errors":[{{"code":{code},"message":"m"}}]}}"#)).unwrap_err()
        };
        assert_eq!(e(32), XError::Unauthorized { status: 200, code: Some(32) });
        assert!(matches!(e(88), XError::RateLimited { .. }));
        assert_eq!(e(353), XError::TransactionIdRequired);
        assert_eq!(e(144), XError::Api { code: 144, message: "m".into() });
    }

    #[test]
    fn statuses_are_mapped() {
        assert_eq!(interp(401, ""), Err(XError::Unauthorized { status: 401, code: None }));
        assert_eq!(interp(403, r#"{"errors":[{"code":353}]}"#), Err(XError::TransactionIdRequired));
        assert_eq!(
            interp(403, r#"{"errors":[{"code":162,"message":"blocked"}]}"#),
            Err(XError::Api { code: 162, message: "blocked".into() }),
            "a refused follow is not a lost session"
        );
        assert_eq!(
            interp(403, r#"{"errors":[{"code":64}]}"#),
            Err(XError::Unauthorized { status: 403, code: Some(64) })
        );
        assert!(matches!(interp(404, "nope"), Err(XError::Http { status: 404, .. })));
        assert!(matches!(interp(429, ""), Err(XError::RateLimited { .. })));
        assert!(matches!(interp(200, "<html>"), Err(XError::Decode(_))));
    }

    #[test]
    fn snippet_is_bounded() {
        assert_eq!(snippet(&"a".repeat(500)).chars().count(), 201);
        assert_eq!(snippet("short"), "short");
    }
}
