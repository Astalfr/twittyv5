//! GraphQL calls on top of [`Transport`]: resolves query ids, builds the
//! request, and self-heals once when Twitter rotated an id.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

use chrono::Utc;
use serde_json::{Value, json};

use crate::{
    Session, XError,
    catalog::{Catalog, Method, OperationSpec, Origin},
    discovery::{Discovery, Report},
    transport::{ApiRequest, Body, ClientIds, RetryPolicy, RotateHook, Transport},
};

/// Client for Twitter's internal GraphQL API. Returns raw JSON; translation into
/// `twitty-model` happens in a separate layer.
#[derive(Debug)]
pub struct XClient {
    transport: Transport,
    catalog: Arc<Catalog>,
    discovery: Option<Discovery>,
    refreshing: tokio::sync::Mutex<()>,
    /// Queries Twitter only serves over POST (learned from a GET 404 + POST success).
    post_only: Mutex<HashSet<String>>,
}

impl XClient {
    /// `discovery: None` disables query-id refresh (tests, offline use).
    pub fn new(transport: Transport, catalog: Arc<Catalog>, discovery: Option<Discovery>) -> Self {
        Self {
            transport,
            catalog,
            discovery,
            refreshing: tokio::sync::Mutex::new(()),
            post_only: Mutex::new(HashSet::new()),
        }
    }

    /// A client against x.com with the production defaults: query-id cache and
    /// client ids under the user's cache directory, live discovery enabled.
    /// `on_rotate` receives the session whenever Twitter rotates a cookie.
    pub fn production(session: Session, on_rotate: Option<RotateHook>) -> Result<Self, XError> {
        let catalog = Arc::new(Catalog::new(Catalog::default_cache_path()));
        let ids_path = directories::ProjectDirs::from("", "", "Twitty")
            .map(|d| d.cache_dir().join("client-ids.json"));
        let ids = ids_path.as_deref().map_or_else(ClientIds::random, ClientIds::load_or_create);
        let base = url::Url::parse(Transport::API_BASE)
            .map_err(|e| XError::Decode(format!("API base: {e}")))?;
        let signing_page = url::Url::parse(Transport::SIGNING_PAGE)
            .map_err(|e| XError::Decode(format!("signing page: {e}")))?;
        let mut transport = Transport::new(session, &ids, base, RetryPolicy::default())?
            .signing_from(signing_page)?;
        if let Some(hook) = on_rotate {
            transport = transport.on_rotate(hook);
        }
        Ok(Self::new(transport, catalog, Some(Discovery::default_pages()?)))
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Re-discovers query ids from x.com and stores them in the catalog cache.
    pub async fn refresh_ids(&self) -> Result<Report, XError> {
        let discovery = self
            .discovery
            .as_ref()
            .ok_or_else(|| XError::Discovery("discovery is disabled".into()))?;
        let _guard = self.refreshing.lock().await;
        let session = self.transport.session();
        let targets = self.catalog.embedded_names();
        let (ops, report) = discovery.run(Some(&session), &targets).await?;
        let changed = self
            .catalog
            .merge_discovered(ops, Utc::now())
            .map_err(|e| XError::Discovery(format!("saving query-id cache failed: {e}")))?;
        tracing::info!(changed, ?report, "query ids refreshed");
        Ok(report)
    }

    /// Calls `operation` with `variables`.
    ///
    /// HTTP 404 has two known causes, handled in this order:
    /// 1. some queries are only served over POST (e.g. `SearchTimeline`): a GET
    ///    404 is retried once as POST, and POST is remembered for the operation;
    /// 2. the query id rotated: ids are refreshed once and the call is retried
    ///    once if a different id was found.
    pub async fn graphql(&self, operation: &str, variables: Value) -> Result<Value, XError> {
        let (spec, origin) = self
            .catalog
            .resolve(operation)
            .ok_or_else(|| XError::UnknownOperation(operation.to_owned()))?;
        match self.call_with_method_fallback(operation, &spec, &variables).await {
            Err(XError::Http { status: 404, .. }) => {
                let stale = || XError::StaleQueryId {
                    operation: operation.to_owned(),
                    query_id: spec.query_id.clone(),
                };
                if origin == Origin::Override || self.discovery.is_none() {
                    return Err(stale());
                }
                if let Err(e) = self.refresh_ids().await {
                    tracing::warn!("query-id refresh after 404 failed: {e}");
                    return Err(stale());
                }
                match self.catalog.resolve(operation) {
                    Some((fresh, _)) if fresh.query_id != spec.query_id => {
                        tracing::info!(operation, "retrying with refreshed query id");
                        self.call_with_method_fallback(operation, &fresh, &variables).await.map_err(
                            |e| match e {
                                XError::Http { status: 404, .. } => XError::StaleQueryId {
                                    operation: operation.to_owned(),
                                    query_id: fresh.query_id.clone(),
                                },
                                other => other,
                            },
                        )
                    }
                    _ => Err(stale()),
                }
            }
            other => other,
        }
    }

    async fn call_with_method_fallback(
        &self,
        operation: &str,
        spec: &OperationSpec,
        variables: &Value,
    ) -> Result<Value, XError> {
        let learned_post =
            self.post_only.lock().unwrap_or_else(PoisonError::into_inner).contains(operation);
        if spec.method == Method::Get && !learned_post {
            match self.call(operation, spec, variables).await {
                Err(XError::Http { status: 404, .. }) => {}
                other => return other,
            }
        }
        let post = OperationSpec { method: Method::Post, ..spec.clone() };
        let result = self.call(operation, &post, variables).await;
        if result.is_ok() && spec.method == Method::Get && !learned_post {
            tracing::info!(operation, "query is served over POST only; remembering");
            self.post_only
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(operation.to_owned());
        }
        result
    }

    async fn call(
        &self,
        operation: &str,
        spec: &OperationSpec,
        variables: &Value,
    ) -> Result<Value, XError> {
        let features = self.catalog.features_for(spec);
        let toggles = self.catalog.field_toggles_for(spec);
        let path = format!("graphql/{}/{operation}", spec.query_id);
        let request = match spec.method {
            Method::Get => {
                let mut query = vec![
                    ("variables".to_owned(), variables.to_string()),
                    ("features".to_owned(), features.to_string()),
                ];
                if let Some(t) = toggles {
                    query.push(("fieldToggles".to_owned(), t.to_string()));
                }
                ApiRequest {
                    endpoint: operation.to_owned(),
                    method: Method::Get,
                    path,
                    query,
                    body: None,
                }
            }
            Method::Post => {
                let mut body = json!({
                    "variables": variables,
                    "features": features,
                    "queryId": spec.query_id,
                });
                if let (Some(t), Some(obj)) = (toggles, body.as_object_mut()) {
                    obj.insert("fieldToggles".to_owned(), t);
                }
                ApiRequest {
                    endpoint: operation.to_owned(),
                    method: Method::Post,
                    path,
                    query: Vec::new(),
                    body: Some(Body::Json(body)),
                }
            }
        };
        self.transport.send(&request).await
    }

    /// Tweets `fields` as a form to the v1.1 REST endpoint at `path` (e.g.
    /// `1.1/friendships/create.json`). Twitter's web client still uses these for
    /// some actions; they have no query id to refresh.
    pub async fn rest_post(
        &self,
        path: &str,
        fields: Vec<(String, String)>,
    ) -> Result<Value, XError> {
        let request = ApiRequest {
            endpoint: path.to_owned(),
            method: Method::Post,
            path: path.to_owned(),
            query: Vec::new(),
            body: Some(Body::Form(fields)),
        };
        self.transport.send(&request).await
    }

    /// The home timeline: "For you" (`latest = false`) or "Following" (`latest = true`).
    pub async fn home_timeline(
        &self,
        latest: bool,
        count: u32,
        cursor: Option<&str>,
    ) -> Result<Value, XError> {
        let operation = if latest { "HomeLatestTimeline" } else { "HomeTimeline" };
        self.graphql(operation, home_variables(latest, count, cursor)).await
    }
}

/// Variables the web client sends for the home timelines.
pub(crate) fn home_variables(latest: bool, count: u32, cursor: Option<&str>) -> Value {
    let mut v = json!({
        "count": count,
        "includePromotedContent": true,
        "latestControlAvailable": true,
        "requestContext": "launch",
        "withCommunity": true,
    });
    if let Some(obj) = v.as_object_mut() {
        if latest {
            obj.insert("seenTweetIds".to_owned(), json!([]));
        }
        if let Some(c) = cursor {
            obj.insert("cursor".to_owned(), json!(c));
            obj.remove("requestContext");
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_variables_shape() {
        let v = home_variables(false, 20, None);
        assert_eq!(v["count"], 20);
        assert_eq!(v["requestContext"], "launch");
        assert!(v.get("cursor").is_none());
        let v = home_variables(true, 40, Some("DAAB"));
        assert_eq!(v["cursor"], "DAAB");
        assert!(v.get("requestContext").is_none(), "paging requests carry no launch context");
        assert_eq!(v["seenTweetIds"], json!([]));
    }
}
