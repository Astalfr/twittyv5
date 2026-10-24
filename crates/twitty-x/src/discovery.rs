//! Best-effort discovery of GraphQL query ids from x.com's JavaScript bundles.
//!
//! X ships two web clients (as of 2026-10):
//! - the legacy `responsive-web/client-web` bundles, with objects like
//!   `{queryId:"…",operationName:"HomeTimeline",operationType:"query",metadata:{featureSwitches:[…],fieldToggles:[…]}}`;
//! - the newer Relay-based `x-web` bundles, with persisted queries like
//!   `params:{id:"…",metadata:{},name:"HomeLatestTimelineQuery",operationKind:"query",…}`.
//!
//! Both formats are extracted. Discovery never blocks the app: when it finds
//! nothing, the cached or embedded catalog is used. See `docs/x-api.md`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::{
    XError,
    catalog::{Method, OperationSpec},
    session::Session,
    transport::CHROME_UA,
};

/// Pages whose HTML references the client bundles.
pub const DEFAULT_PAGES: &[&str] = &["https://x.com/home", "https://x.com/?lang=en"];
/// Upper bound on scripts fetched in one discovery run.
const MAX_SCRIPTS: usize = 250;
/// Concurrent downloads.
const CONCURRENCY: usize = 8;

/// Which bundle format an operation was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    Legacy,
    Relay,
}

/// An operation found in a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub spec: OperationSpec,
    pub format: Format,
}

/// Summary of a discovery run, safe to print.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Scripts fetched, by family (`responsive-web`, `x-web`, `other`).
    pub scripts_by_family: BTreeMap<String, usize>,
    /// Operations found, by format.
    pub ops_by_format: BTreeMap<String, usize>,
    /// Pages that could not be fetched.
    pub failed_pages: Vec<String>,
}

const Q: &str = r#"["'`]"#;

static LEGACY: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"queryId:{Q}(?P<id>[A-Za-z0-9_-]+){Q},operationName:{Q}(?P<name>\w+){Q},operationType:{Q}(?P<kind>\w+){Q}(?:,metadata:\{{featureSwitches:\[(?P<fs>[^\]]*)\],fieldToggles:\[(?P<ft>[^\]]*)\])?"
    ))
});

static RELAY: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"params:\{{id:{Q}(?P<id>[A-Za-z0-9_-]+){Q},metadata:\{{[^}}]*\}},name:{Q}(?P<name>\w+){Q},operationKind:{Q}(?P<kind>query|mutation|subscription){Q}"
    ))
});

static SCRIPT_SRC: LazyLock<Regex> =
    LazyLock::new(|| re(r#"<script[^>]*\ssrc=["']([^"']+\.js)["']"#));
static ABS_JS: LazyLock<Regex> =
    LazyLock::new(|| re(r#"https://abs\.twimg\.com/[A-Za-z0-9/_.~-]+\.js"#));
static ASSET_REF: LazyLock<Regex> = LazyLock::new(|| re(r#"assets/[A-Za-z0-9_.-]+\.js"#));
static NAME_LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"{Q}(\w+){Q}")));
/// Lazily loaded chunks worth fetching (the rest are locales, settings, ads…).
static RELEVANT_ASSET: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(graphql|timeline|home|tweet|post|search|bookmark|notif|list|profile|user|conversation|status|favorite|retweet|composer|startup)",
    )
});

#[expect(clippy::expect_used, reason = "patterns are compile-time constants covered by tests")]
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// Every operation declared in a bundle, in either format.
pub fn extract_operations(js: &str) -> Vec<Found> {
    let list = |s: Option<regex::Match<'_>>| {
        s.map(|m| {
            NAME_LIST_ITEM.captures_iter(m.as_str()).map(|c| c[1].to_owned()).collect::<Vec<_>>()
        })
    };
    let method = |kind: &str| if kind == "mutation" { Method::Post } else { Method::Get };
    let legacy = LEGACY.captures_iter(js).map(|c| Found {
        name: c["name"].to_owned(),
        spec: OperationSpec {
            query_id: c["id"].to_owned(),
            method: method(&c["kind"]),
            feature_switches: list(c.name("fs")),
            field_toggles: list(c.name("ft")),
            source: "discovered (legacy bundle)".into(),
        },
        format: Format::Legacy,
    });
    let relay = RELAY.captures_iter(js).map(|c| Found {
        name: c["name"].to_owned(),
        spec: OperationSpec {
            query_id: c["id"].to_owned(),
            method: method(&c["kind"]),
            feature_switches: None,
            field_toggles: None,
            source: "discovered (relay bundle)".into(),
        },
        format: Format::Relay,
    });
    legacy.chain(relay).collect()
}

/// Script URLs referenced by a page, resolved against `page`.
pub fn extract_script_urls(html: &str, page: &Url) -> Vec<Url> {
    let srcs = SCRIPT_SRC.captures_iter(html).filter_map(|c| page.join(&c[1]).ok());
    let abs = ABS_JS.find_iter(html).filter_map(|m| Url::parse(m.as_str()).ok());
    dedup(srcs.chain(abs))
}

/// Relevant lazily loaded chunks a script refers to as `assets/<name>.js`.
///
/// These paths are relative to the bundle base (the directory holding
/// `assets/`), not to the referring file: a chunk at `…/x-web/assets/a.js`
/// that mentions `assets/b.js` means `…/x-web/assets/b.js`.
pub fn extract_asset_urls(js: &str, script: &Url) -> Vec<Url> {
    let base = bundle_base(script);
    dedup(
        ASSET_REF
            .find_iter(js)
            .filter(|m| RELEVANT_ASSET.is_match(m.as_str()))
            .filter_map(|m| base.join(m.as_str()).ok()),
    )
}

/// The directory `assets/` paths are relative to.
fn bundle_base(script: &Url) -> Url {
    let path = script.path();
    match path.rfind("/assets/") {
        Some(ix) => {
            let mut base = script.clone();
            base.set_path(&path[..=ix]);
            base
        }
        None => script.clone(),
    }
}

fn dedup(urls: impl Iterator<Item = Url>) -> Vec<Url> {
    let mut seen = HashSet::new();
    urls.filter(|u| seen.insert(u.to_string())).collect()
}

fn family(url: &Url) -> &'static str {
    let path = url.path();
    if path.contains("/responsive-web/") {
        "responsive-web"
    } else if path.contains("/x-web/") {
        "x-web"
    } else {
        "other"
    }
}

/// Downloads pages and bundles and extracts operations.
#[derive(Debug, Clone)]
pub struct Discovery {
    http: reqwest::Client,
    pages: Vec<Url>,
}

impl Discovery {
    /// Discovery over `pages` (use [`DEFAULT_PAGES`] in production).
    pub fn new(pages: Vec<Url>) -> Result<Self, XError> {
        let http = reqwest::Client::builder()
            .user_agent(CHROME_UA)
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self { http, pages })
    }

    /// Discovery over the default x.com pages.
    pub fn default_pages() -> Result<Self, XError> {
        Self::new(DEFAULT_PAGES.iter().filter_map(|p| Url::parse(p).ok()).collect())
    }

    /// Runs discovery. With a session, pages are fetched logged in, which is
    /// what exposes the logged-in client's bundles.
    pub async fn run(
        &self,
        session: Option<&Session>,
    ) -> Result<(HashMap<String, OperationSpec>, Report), XError> {
        let mut report = Report::default();
        let mut scripts: Vec<Url> = Vec::new();
        for page in &self.pages {
            match self.fetch(page, session).await {
                Ok(html) => scripts.extend(extract_script_urls(&html, page)),
                Err(e) => {
                    tracing::warn!("discovery: {page} failed: {e}");
                    report.failed_pages.push(page.to_string());
                }
            }
        }
        if scripts.is_empty() {
            return Err(XError::Discovery("no script bundles found on any page".into()));
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = dedup(scripts.into_iter());
        let mut found: HashMap<String, OperationSpec> = HashMap::new();
        while !queue.is_empty() && seen.len() < MAX_SCRIPTS {
            let batch: Vec<Url> = queue
                .drain(..queue.len().min(CONCURRENCY))
                .filter(|u| seen.insert(u.to_string()))
                .collect();
            let mut set = tokio::task::JoinSet::new();
            for url in batch {
                let this = self.clone();
                set.spawn(async move {
                    let body = this.fetch(&url, None).await;
                    (url, body)
                });
            }
            while let Some(joined) = set.join_next().await {
                let Ok((url, Ok(js))) = joined else { continue };
                *report.scripts_by_family.entry(family(&url).to_owned()).or_default() += 1;
                for op in extract_operations(&js) {
                    *report.ops_by_format.entry(format!("{:?}", op.format)).or_default() += 1;
                    found.entry(op.name).or_insert(op.spec);
                }
                queue.extend(
                    extract_asset_urls(&js, &url)
                        .into_iter()
                        .filter(|u| !seen.contains(u.as_str())),
                );
            }
        }
        Ok((found, report))
    }

    async fn fetch(&self, url: &Url, session: Option<&Session>) -> Result<String, XError> {
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

    #[test]
    fn legacy_objects_with_metadata() {
        let js = r#"e.exports={queryId:"0toR0MxC06yY3Fc57GXBpA",operationName:"HomeTimeline",operationType:"query",metadata:{featureSwitches:["a_enabled","b_enabled"],fieldToggles:["withArticleRichContentState"]}}"#;
        let ops = extract_operations(js);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "HomeTimeline");
        assert_eq!(ops[0].format, Format::Legacy);
        assert_eq!(ops[0].spec.query_id, "0toR0MxC06yY3Fc57GXBpA");
        assert_eq!(ops[0].spec.method, Method::Get);
        assert_eq!(
            ops[0].spec.feature_switches.as_deref(),
            Some(&["a_enabled".to_owned(), "b_enabled".to_owned()][..])
        );
        assert_eq!(
            ops[0].spec.field_toggles.as_deref(),
            Some(&["withArticleRichContentState".to_owned()][..])
        );
    }

    #[test]
    fn legacy_mutation_without_metadata_and_backtick_quotes() {
        let js = "x={queryId:`lI07N6Otwv1PhnEgXILM7A`,operationName:`FavoriteTweet`,operationType:`mutation`}";
        let ops = extract_operations(js);
        assert_eq!(ops[0].spec.method, Method::Post);
        assert_eq!(ops[0].spec.feature_switches, None);
    }

    #[test]
    fn relay_persisted_queries() {
        let js = "storageKey:null}]},params:{id:`AKtIqG462iRt7PwVMbHliQ`,metadata:{},name:`HomeLatestTimelineQuery`,operationKind:`query`,text:null}}";
        let ops = extract_operations(js);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "HomeLatestTimelineQuery");
        assert_eq!(ops[0].format, Format::Relay);
        assert_eq!(ops[0].spec.query_id, "AKtIqG462iRt7PwVMbHliQ");
        let m = "params:{id:`xpr9WEPyysxG624ZF0FEeg`,metadata:{},name:`shareMutation`,operationKind:`mutation`,text:null}";
        assert_eq!(extract_operations(m)[0].spec.method, Method::Post);
    }

    #[test]
    fn unrelated_code_yields_nothing() {
        let js = r#"return{queryId:n[1],operationName:n[2]??`<unknown>`,method:r}"#;
        assert!(extract_operations(js).is_empty());
    }

    #[test]
    fn script_urls_from_html() {
        let page = Url::parse("https://x.com/home").unwrap();
        let html = r#"<script type="module" src="https://abs.twimg.com/x-web/x-web/entry-A.js"></script>
            <script src="/rel/main.js"></script>
            <link rel="preload" href="https://abs.twimg.com/responsive-web/client-web/main.abc.js">"#;
        let urls: Vec<String> =
            extract_script_urls(html, &page).iter().map(ToString::to_string).collect();
        assert_eq!(
            urls,
            [
                "https://abs.twimg.com/x-web/x-web/entry-A.js",
                "https://x.com/rel/main.js",
                "https://abs.twimg.com/responsive-web/client-web/main.abc.js",
            ]
        );
    }

    #[test]
    fn asset_urls_keep_only_relevant_chunks() {
        let script = Url::parse("https://abs.twimg.com/x-web/x-web/entry-A.js").unwrap();
        let js = r#"import("assets/home-BYGUKMSn.js");f("assets/de-CVIJcuCb.js");g("assets/HomeQuery.graphql-X1.js")"#;
        let urls: Vec<String> =
            extract_asset_urls(js, &script).iter().map(ToString::to_string).collect();
        assert_eq!(
            urls,
            [
                "https://abs.twimg.com/x-web/x-web/assets/home-BYGUKMSn.js",
                "https://abs.twimg.com/x-web/x-web/assets/HomeQuery.graphql-X1.js",
            ]
        );
    }

    #[test]
    fn asset_refs_inside_chunks_resolve_against_bundle_base() {
        let chunk = Url::parse("https://abs.twimg.com/x-web/x-web/assets/startup-Bh.js").unwrap();
        let urls = extract_asset_urls(r#"x("assets/home-C3.js")"#, &chunk);
        assert_eq!(urls[0].as_str(), "https://abs.twimg.com/x-web/x-web/assets/home-C3.js");
    }

    #[test]
    fn families() {
        let f = |s: &str| family(&Url::parse(s).unwrap());
        assert_eq!(f("https://abs.twimg.com/responsive-web/client-web/main.js"), "responsive-web");
        assert_eq!(f("https://abs.twimg.com/x-web/x-web/entry.js"), "x-web");
        assert_eq!(f("https://example.com/a.js"), "other");
    }
}
