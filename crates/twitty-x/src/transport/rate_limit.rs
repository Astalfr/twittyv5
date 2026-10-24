//! Per-operation rate-limit tracking from `x-rate-limit-*` headers.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use chrono::{DateTime, TimeZone, Utc};
use reqwest::header::HeaderMap;

/// The last observed budget of one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: u32,
    pub remaining: u32,
    pub reset_at: DateTime<Utc>,
}

impl RateLimit {
    /// Parses the three `x-rate-limit-*` headers; `None` if any is missing.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let num = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
        let reset_at = Utc.timestamp_opt(num("x-rate-limit-reset")?, 0).single()?;
        Some(Self {
            limit: u32::try_from(num("x-rate-limit-limit")?).ok()?,
            remaining: u32::try_from(num("x-rate-limit-remaining")?).ok()?,
            reset_at,
        })
    }
}

/// Budgets keyed by operation name. Each operation has its own window at X.
#[derive(Debug, Default)]
pub struct RateLimits {
    by_endpoint: Mutex<HashMap<String, RateLimit>>,
}

impl RateLimits {
    /// Records the budget reported by a response.
    pub fn observe(&self, endpoint: &str, headers: &HeaderMap) {
        if let Some(limit) = RateLimit::from_headers(headers) {
            self.by_endpoint
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(endpoint.to_owned(), limit);
        }
    }

    /// The last known budget of `endpoint`.
    pub fn get(&self, endpoint: &str) -> Option<RateLimit> {
        self.by_endpoint.lock().unwrap_or_else(PoisonError::into_inner).get(endpoint).copied()
    }

    /// `Err(reset_at)` when the budget is exhausted and the window has not reset yet.
    pub fn check(&self, endpoint: &str, now: DateTime<Utc>) -> Result<(), DateTime<Utc>> {
        match self.get(endpoint) {
            Some(l) if l.remaining == 0 && l.reset_at > now => Err(l.reset_at),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(limit: &str, remaining: &str, reset: i64) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-rate-limit-limit", HeaderValue::from_str(limit).unwrap());
        h.insert("x-rate-limit-remaining", HeaderValue::from_str(remaining).unwrap());
        h.insert("x-rate-limit-reset", HeaderValue::from_str(&reset.to_string()).unwrap());
        h
    }

    #[test]
    fn parses_headers() {
        let l = RateLimit::from_headers(&headers("500", "499", 1_800_000_000)).unwrap();
        assert_eq!((l.limit, l.remaining, l.reset_at.timestamp()), (500, 499, 1_800_000_000));
    }

    #[test]
    fn missing_or_garbage_headers_are_ignored() {
        assert!(RateLimit::from_headers(&HeaderMap::new()).is_none());
        assert!(RateLimit::from_headers(&headers("x", "1", 1)).is_none());
        assert!(RateLimit::from_headers(&headers("5", "-1", 1)).is_none());
    }

    #[test]
    fn exhausted_budget_blocks_until_reset_per_endpoint() {
        let limits = RateLimits::default();
        let now = Utc::now();
        let reset = now.timestamp() + 60;
        limits.observe("HomeTimeline", &headers("500", "0", reset));
        assert_eq!(limits.check("HomeTimeline", now).unwrap_err().timestamp(), reset);
        assert!(limits.check("SearchTimeline", now).is_ok(), "other endpoints unaffected");
        let later = Utc.timestamp_opt(reset + 1, 0).unwrap();
        assert!(limits.check("HomeTimeline", later).is_ok(), "window reset");
    }
}
