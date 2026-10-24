//! `x-client-transaction-id`: the per-request signature Twitter's web client sends.
//!
//! The web client derives it from a key in the home page
//! (`twitter-site-verification`), one of four "loading" SVG animations in the
//! same page, and byte indices hidden in the `ondemand.s` chunk; then it hashes
//! method, path and time with them. Without it Twitter refuses some writes (error
//! 226, "looks like it might be automated"). Ported from XClientTransaction
//! (MIT); see `docs/x-api.md`.

use std::{
    fmt,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use chrono::{DateTime, Utc};
use regex::Regex;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{XError, catalog::Method, discovery, session::Session};

/// Seconds Twitter counts from (2023-05-01 07:00 UTC).
const EPOCH_SECS: i64 = 1_682_924_400;
const KEYWORD: &str = "obfiowerehiring";
const EXTRA_BYTE: u8 = 3;
/// Length of the loading animation in ms.
const TOTAL_TIME: f64 = 4096.0;
/// How long a key is used before the page is fetched again.
const KEY_TTL: Duration = Duration::from_secs(60 * 60);
/// How long to wait before trying again after the key could not be built.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

static META: LazyLock<Regex> =
    LazyLock::new(|| re(r#"<meta[^>]*name=["']twitter-site-verification["'][^>]*>"#));
static CONTENT: LazyLock<Regex> = LazyLock::new(|| re(r#"content=["']([^"']+)["']"#));
static INDEX: LazyLock<Regex> = LazyLock::new(|| re(r"\(\w\[(\d{1,2})\],\s*16\)"));
static PATH_D: LazyLock<Regex> = LazyLock::new(|| re(r#"<path[^>]*\sd=["']([^"']*)["']"#));

#[expect(clippy::expect_used, reason = "patterns are compile-time constants covered by tests")]
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// Why the signing key could not be built from the page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum KeyError {
    #[error("no twitter-site-verification key in the page")]
    NoVerification,
    #[error("the verification key is not base64")]
    BadVerification,
    #[error("no key byte indices in the ondemand.s chunk")]
    NoIndices,
    #[error("a key byte index is out of range")]
    IndexOutOfRange,
    #[error("no ondemand.s chunk in the page")]
    NoOndemand,
    #[error("the loading animation is missing or malformed")]
    BadAnimation,
}

/// What a transaction id is derived from; valid for one page load.
#[derive(Clone, PartialEq)]
pub(crate) struct TransactionKey {
    key: Vec<u8>,
    animation: String,
}

impl fmt::Debug for TransactionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Session-bound material: don't print it.
        f.debug_struct("TransactionKey").finish_non_exhaustive()
    }
}

impl TransactionKey {
    /// Builds the key from the home page HTML and the `ondemand.s` chunk.
    pub(crate) fn from_page(html: &str, ondemand_js: &str) -> Result<Self, KeyError> {
        let meta = META.find(html).ok_or(KeyError::NoVerification)?;
        let content = CONTENT.captures(meta.as_str()).ok_or(KeyError::NoVerification)?;
        let key = STANDARD.decode(&content[1]).map_err(|_| KeyError::BadVerification)?;

        let indices: Vec<usize> =
            INDEX.captures_iter(ondemand_js).filter_map(|c| c[1].parse().ok()).collect();
        let (&row_ix, frame_ixs) = indices.split_first().ok_or(KeyError::NoIndices)?;
        if frame_ixs.is_empty() {
            return Err(KeyError::NoIndices);
        }
        let byte = |i: usize| key.get(i).copied().ok_or(KeyError::IndexOutOfRange);

        let frames = animation_rows(html, usize::from(byte(5)? % 4))?;
        let row = frames.get(usize::from(byte(row_ix)? % 16)).ok_or(KeyError::BadAnimation)?;
        let mut frame_time = 1.0;
        for &i in frame_ixs {
            frame_time *= f64::from(byte(i)? % 16);
        }
        let frame_time = js_round(frame_time / 10.0) * 10.0;
        let animation = animate(row, frame_time / TOTAL_TIME)?;
        Ok(Self { key, animation })
    }

    /// The id for a request; `now` and `random` are parameters for tests.
    pub(crate) fn id(&self, method: Method, path: &str, now: DateTime<Utc>, random: u8) -> String {
        let method = match method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let time = (now.timestamp_millis() - EPOCH_SECS * 1000).div_euclid(1000);
        let hash = Sha256::digest(format!("{method}!{path}!{time}{KEYWORD}{}", self.animation));
        let mut out = vec![random];
        out.extend(
            self.key
                .iter()
                .copied()
                .chain((0..4).map(|i| ((time >> (i * 8)) & 0xFF) as u8))
                .chain(hash[..16].iter().copied())
                .chain([EXTRA_BYTE])
                .map(|b| b ^ random),
        );
        STANDARD_NO_PAD.encode(out)
    }
}

/// The keyframe rows of `loading-x-anim-<n>`: its second path's `d`, after
/// the `M 10,30 C` prefix, split on `C`, one row of numbers per segment.
fn animation_rows(html: &str, n: usize) -> Result<Vec<Vec<f64>>, KeyError> {
    let start = html.find(&format!("id=\"loading-x-anim-{n}\"")).ok_or(KeyError::BadAnimation)?;
    let svg = &html[start..];
    let svg = &svg[..svg.find("</svg>").ok_or(KeyError::BadAnimation)?];
    let d = PATH_D.captures_iter(svg).nth(1).ok_or(KeyError::BadAnimation)?;
    let rows = d[1].get(9..).ok_or(KeyError::BadAnimation)?;
    Ok(rows
        .split('C')
        .map(|seg| {
            seg.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<f64>().ok()).collect()
        })
        .collect())
}

/// The animation state at `time` (0–1) as a hex string: colour, then the
/// rotation matrix.
fn animate(row: &[f64], time: f64) -> Result<String, KeyError> {
    if row.len() < 11 {
        return Err(KeyError::BadAnimation);
    }
    let curves: Vec<f64> = row[7..11]
        .iter()
        .enumerate()
        .map(|(i, &v)| solve(v, if i % 2 == 1 { -1.0 } else { 0.0 }, 1.0, false))
        .collect();
    let val = cubic(&curves, time);
    let color = (0..3).map(|i| lerp(row[i], row[i + 3], val).max(0.0));
    let rotation = lerp(0.0, solve(row[6], 60.0, 360.0, true), val).to_radians();
    let (sin, cos) = rotation.sin_cos();

    let mut out: String = color.map(|c| format!("{:x}", c.round_ties_even() as u64)).collect();
    for m in [cos, -sin, sin, cos] {
        let hex = float_to_hex(((m * 100.0).round() / 100.0).abs());
        if hex.starts_with('.') {
            out.push('0');
            out.push_str(&hex.to_lowercase());
        } else {
            out.push_str(if hex.is_empty() { "0" } else { &hex });
        }
    }
    out.push_str("00");
    Ok(out.replace(['.', '-'], ""))
}

fn solve(value: f64, min: f64, max: f64, floor: bool) -> f64 {
    let v = value * (max - min) / 255.0 + min;
    if floor { v.floor() } else { (v * 100.0).round() / 100.0 }
}

fn lerp(from: f64, to: f64, f: f64) -> f64 {
    from * (1.0 - f) + to * f
}

/// JavaScript's `Math.round`: halves round up.
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

/// CSS cubic-bezier easing with control points `c` = [x1, y1, x2, y2].
fn cubic(c: &[f64], time: f64) -> f64 {
    let bezier = |a: f64, b: f64, m: f64| {
        3.0 * a * (1.0 - m) * (1.0 - m) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
    };
    if time <= 0.0 {
        let gradient = if c[0] > 0.0 {
            c[1] / c[0]
        } else if c[1] == 0.0 && c[2] > 0.0 {
            c[3] / c[2]
        } else {
            0.0
        };
        return gradient * time;
    }
    if time >= 1.0 {
        let gradient = if c[2] < 1.0 {
            (c[3] - 1.0) / (c[2] - 1.0)
        } else if c[2] == 1.0 && c[0] < 1.0 {
            (c[1] - 1.0) / (c[0] - 1.0)
        } else {
            0.0
        };
        return 1.0 + gradient * (time - 1.0);
    }
    let (mut start, mut end, mut mid) = (0.0, 1.0, 0.0);
    // Bisection; 100 halvings exhaust f64 precision, so this always ends.
    for _ in 0..100 {
        if start >= end {
            break;
        }
        mid = (start + end) / 2.0;
        let x = bezier(c[0], c[2], mid);
        if (time - x).abs() < 0.00001 {
            break;
        }
        if x < time {
            start = mid;
        } else {
            end = mid;
        }
    }
    bezier(c[1], c[3], mid)
}

/// A non-negative float in hex, uppercase digits, as the web client prints it
/// (`0.5` → `.8`, `1` → `1`, `0` → empty).
fn float_to_hex(mut x: f64) -> String {
    let digit = |d: u32| char::from_digit(d, 16).unwrap_or('0').to_ascii_uppercase();
    let mut int_digits = Vec::new();
    let mut quotient = x.trunc();
    let mut fraction = x - quotient;
    while quotient > 0.0 {
        quotient = (x / 16.0).trunc();
        int_digits.push(digit((x - quotient * 16.0).trunc() as u32));
        x = quotient;
    }
    let mut out: String = int_digits.iter().rev().collect();
    if fraction > 0.0 {
        out.push('.');
        while fraction > 0.0 {
            fraction *= 16.0;
            let d = fraction.trunc();
            fraction -= d;
            out.push(digit(d as u32));
        }
    }
    out
}

/// A random 32-hex value, sent when no key could be built.
pub(crate) fn random_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn random_byte() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}

/// Signs requests with keys fetched from x.com, cached for [`KEY_TTL`].
pub(crate) struct Signer {
    http: reqwest::Client,
    page: Url,
    state: tokio::sync::Mutex<Option<Loaded>>,
}

struct Loaded {
    key: Option<Arc<TransactionKey>>,
    at: Instant,
}

impl fmt::Debug for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signer").field("page", &self.page.as_str()).finish_non_exhaustive()
    }
}

impl Signer {
    /// A signer that reads its key from `page` (`https://x.com/home` in production).
    pub(crate) fn new(page: Url) -> Result<Self, XError> {
        let http = reqwest::Client::builder()
            .user_agent(super::CHROME_UA)
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { http, page, state: tokio::sync::Mutex::new(None) })
    }

    /// The id for a request to `path` (the URL path, e.g. `/i/api/graphql/…`).
    /// Falls back to a random value when the key can't be built.
    pub(crate) async fn id(&self, session: &Session, method: Method, path: &str) -> String {
        match self.key(session).await {
            Some(key) => key.id(method, path, Utc::now(), random_byte()),
            None => random_id(),
        }
    }

    async fn key(&self, session: &Session) -> Option<Arc<TransactionKey>> {
        let mut state = self.state.lock().await;
        let expired = state
            .as_ref()
            .is_none_or(|l| l.at.elapsed() > if l.key.is_some() { KEY_TTL } else { RETRY_AFTER });
        if expired {
            let key = match self.fetch(session).await {
                Ok(key) => {
                    tracing::debug!("transaction id key loaded");
                    Some(Arc::new(key))
                }
                Err(e) => {
                    tracing::warn!("transaction id key unavailable, sending random ids: {e}");
                    None
                }
            };
            *state = Some(Loaded { key, at: Instant::now() });
        }
        state.as_ref().and_then(|l| l.key.clone())
    }

    async fn fetch(&self, session: &Session) -> Result<TransactionKey, XError> {
        let html = self.get(&self.page, Some(session)).await?;
        let ondemand = discovery::extract_webpack_chunks(&html, &self.page)
            .into_iter()
            .find(|u| u.path().rsplit('/').next().is_some_and(|f| f.starts_with("ondemand.s.")))
            .ok_or_else(|| XError::Decode(KeyError::NoOndemand.to_string()))?;
        let js = self.get(&ondemand, None).await?;
        TransactionKey::from_page(&html, &js).map_err(|e| XError::Decode(e.to_string()))
    }

    async fn get(&self, url: &Url, session: Option<&Session>) -> Result<String, XError> {
        let mut req = self.http.get(url.clone()).header("accept-language", "en-US,en;q=0.9");
        if let Some(s) = session {
            req = req.header("cookie", s.cookie_header());
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(XError::Http { status: resp.status().as_u16(), snippet: url.to_string() });
        }
        Ok(resp.text().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // Synthetic page and chunk in Twitter's format (made-up key and animation).
    // Expected values come from a straight port of XClientTransaction's Python.
    const HOME: &str = include_str!("../../tests/fixtures/transaction/home.html");
    const ONDEMAND: &str = include_str!("../../tests/fixtures/transaction/ondemand.s.js");

    fn key() -> TransactionKey {
        TransactionKey::from_page(HOME, ONDEMAND).unwrap()
    }

    #[test]
    fn animation_key_matches_the_reference() {
        assert_eq!(
            key().animation,
            "8392600e147ae147ae14807ae147ae147ae07ae147ae147ae0e147ae147ae14800"
        );
    }

    #[test]
    fn ids_match_the_reference() {
        let at = |secs: i64| DateTime::from_timestamp(EPOCH_SECS + secs, 0).unwrap();
        let path = "/i/api/graphql/6uvqjlPaij9o7Ce15XW4sA/retweetButtonRetweetMutation";
        assert_eq!(
            key().id(Method::Post, path, at(112_233_445), 7),
            "B10hmg5lGCuqtstWXvXZ/o+Fu6yVIBHRGU+ZVEONK7t9Fpil7RsVyWQu+Pw8H/pYs+KMtwEkPmSjyqR6gcbK/jL9vSb5BA"
        );
        assert_eq!(
            key().id(Method::Get, "/i/api/graphql/x/HomeTimeline", at(1), 255),
            "/6XZYvad4NNSTjOupg0hBnd9Q1Rt2Okp4bdhrLt100OF7mBdFePtMZzWAATE5wKgS/7////u7nFu2JtONYifMPfcIERF/A"
        );
    }

    #[test]
    fn ids_unmask_to_key_time_hash_and_marker() {
        let k = key();
        let id =
            k.id(Method::Post, "/p", DateTime::from_timestamp(EPOCH_SECS + 258, 0).unwrap(), 9);
        let raw = STANDARD_NO_PAD.decode(id).unwrap();
        let bytes: Vec<u8> = raw[1..].iter().map(|b| b ^ raw[0]).collect();
        assert_eq!(raw[0], 9);
        assert_eq!(bytes.len(), k.key.len() + 4 + 16 + 1);
        assert_eq!(&bytes[..k.key.len()], &k.key[..]);
        assert_eq!(&bytes[k.key.len()..k.key.len() + 4], &[2, 1, 0, 0], "258 s, little endian");
        assert_eq!(bytes.last(), Some(&EXTRA_BYTE));
    }

    #[test]
    fn missing_pieces_are_errors() {
        assert_eq!(TransactionKey::from_page("<html>", ONDEMAND), Err(KeyError::NoVerification));
        assert_eq!(TransactionKey::from_page(HOME, "parseInt(n[1],10)"), Err(KeyError::NoIndices));
        assert_eq!(
            TransactionKey::from_page(HOME, "(n[1],16)(n[99],16)"),
            Err(KeyError::IndexOutOfRange)
        );
        let no_frames = HOME.replace("loading-x-anim", "other-anim");
        assert_eq!(TransactionKey::from_page(&no_frames, ONDEMAND), Err(KeyError::BadAnimation));
        let bad_meta = HOME.replace(
            "twitter-site-verification\" content=\"",
            "twitter-site-verification\" content=\"!",
        );
        assert_eq!(TransactionKey::from_page(&bad_meta, ONDEMAND), Err(KeyError::BadVerification));
    }

    #[test]
    fn floats_print_like_the_web_client() {
        assert_eq!(float_to_hex(0.0), "");
        assert_eq!(float_to_hex(1.0), "1");
        assert_eq!(float_to_hex(0.5), ".8");
        assert_eq!(float_to_hex(0.87), ".DEB851EB851EB8");
        assert_eq!(float_to_hex(26.0), "1A");
    }

    #[test]
    fn easing_is_monotonic_on_a_standard_curve() {
        let ease = [0.25, 0.1, 0.25, 1.0];
        assert_eq!(cubic(&ease, 0.0), 0.0);
        assert_eq!(cubic(&ease, 1.0), 1.0);
        let samples: Vec<f64> = (1..10).map(|i| cubic(&ease, f64::from(i) / 10.0)).collect();
        assert!(samples.windows(2).all(|w| w[0] < w[1]), "{samples:?}");
    }

    #[test]
    fn random_ids_are_unique_hex() {
        let a = random_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_id());
    }
}
