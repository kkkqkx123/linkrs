//! Elasticsearch remote backend for fulltext indexes.
//!
//! Implements the same [`FulltextSearchEngine`] trait as the embedded Tantivy
//! engine over the Elasticsearch HTTP API, using the workspace `reqwest`
//! client (no vendor SDK). Because the manager and the sync receiver already
//! hold `Arc<dyn FulltextSearchEngine>`, this backend drops into the existing
//! delivery path without changes to the sync crate.
//!
//! # Document layout
//!
//! Every indexed document carries a `logical_index` keyword (the local
//! index id), a `doc_id` keyword, an analyzed `content` field and a `doc_type`
//! keyword. By default all logical indexes share one ES index and are scoped
//! by `logical_index` filters; `single_index = false` gives each logical index
//! its own ES index instead. The ES `_id` is a URL-safe encoding of
//! `(logical_index, doc_id)`, which makes writes idempotent on retry.
//!
//! # Commit fence
//!
//! ES has no transactional commit payload. The receiver's consistency fence
//! (applied LSN + receipts) is persisted in a reserved system document per
//! logical index. `commit_with_payload` first refreshes pending writes, then
//! stores the payload in that document and refreshes again, so a payload read
//! back always refers to data that is already visible. The fence document is
//! excluded from search and count results by the `doc_type` filter.
//!
//! # Failure handling
//!
//! Transient failures (5xx, 429, 408, transport errors) are retried and
//! recorded by a circuit breaker; configuration failures (other 4xx) are
//! reported without tripping the breaker.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use linkrs_config::fulltext::EsConfig;
use linkrs_core::Value;
use parking_lot::Mutex;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{Value as JsonValue, json};

use crate::engine::FulltextSearchEngine;
use crate::error::SearchError;
use crate::query::FulltextQuery;
use crate::result::{IndexStats, SearchResult};
use crate::ConsistencyState;

const DOC_TYPE_DOCUMENT: &str = "document";
const DOC_TYPE_FENCE: &str = "fence";
const FENCE_DOC_KEY: &str = "__linkrs_commit_fence__";

// Consistency state mirrors the discriminants of ConsistencyState.
const STATE_CONSISTENT: u8 = 0;
const STATE_INCONSISTENT: u8 = 1;
const STATE_REBUILDING: u8 = 2;

/// Minimal sync circuit breaker for the remote backend.
///
/// Kept local to this module so `linkrs-fulltext` does not depend on the sync
/// crate (the dependency DAG forbids it). State transitions are closed → open
/// after `THRESHOLD` transient failures, and a probe is allowed once the open
/// window elapses.
#[derive(Debug)]
struct EsBreaker {
    failures: u32,
    opened_until: Option<Instant>,
}

impl EsBreaker {
    const THRESHOLD: u32 = 5;
    const WINDOW: Duration = Duration::from_secs(30);

    fn new() -> Self {
        Self {
            failures: 0,
            opened_until: None,
        }
    }

    fn check(&self) -> Result<(), SearchError> {
        if let Some(until) = self.opened_until {
            if Instant::now() < until {
                return Err(SearchError::EngineUnavailable);
            }
        }
        Ok(())
    }

    fn record_success(&mut self) {
        self.failures = 0;
        self.opened_until = None;
    }

    fn record_failure(&mut self) {
        self.failures += 1;
        if self.failures >= Self::THRESHOLD {
            self.opened_until = Some(Instant::now() + Self::WINDOW);
        }
    }

    #[cfg(test)]
    fn trip(&mut self) {
        self.opened_until = Some(Instant::now() + Self::WINDOW);
    }
}

#[derive(Debug, Clone, Copy)]
enum Method {
    Get,
    Post,
    Put,
}

#[derive(Debug, Clone)]
enum RequestBody {
    None,
    Json(JsonValue),
    Ndjson(String),
}

/// Elasticsearch-backed fulltext engine (remote branch).
pub struct EsSearchEngine {
    http: Client,
    config: EsConfig,
    /// Sanitized base ES index name from configuration.
    base_index: String,
    /// Local logical index id this engine instance serves.
    logical_index: String,
    breaker: Mutex<EsBreaker>,
    consistency: AtomicU8,
}

impl std::fmt::Debug for EsSearchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsSearchEngine")
            .field("url", &self.config.url)
            .field("base_index", &self.base_index)
            .field("single_index", &self.config.single_index)
            .field("logical_index", &self.logical_index)
            .finish_non_exhaustive()
    }
}

impl EsSearchEngine {
    /// Build an engine instance. Performs no network I/O; the ES index mapping
    /// is created lazily by [`Self::ensure_index`] from the manager's create
    /// path.
    pub fn new(config: EsConfig, logical_index: String) -> Result<Self, SearchError> {
        config
            .validate()
            .map_err(SearchError::ConfigError)?;
        let base_index = Self::sanitize_index_name(&config.index_name);
        let mut builder = Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .user_agent("linkrs-fulltext");
        if let Some(ref api_key) = config.api_key {
            let mut headers = reqwest::header::HeaderMap::new();
            let value = reqwest::header::HeaderValue::from_str(&format!("ApiKey {api_key}"))
                .map_err(|e| SearchError::ConfigError(format!("invalid es api key: {e}")))?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
            builder = builder.default_headers(headers);
        }
        let http = builder
            .build()
            .map_err(|_e| SearchError::EngineUnavailable)?;
        Ok(Self {
            http,
            config,
            base_index,
            logical_index,
            breaker: Mutex::new(EsBreaker::new()),
            consistency: AtomicU8::new(STATE_CONSISTENT),
        })
    }

    /// Lowercase and restrict an index name to ES-legal characters.
    fn sanitize_index_name(name: &str) -> String {
        name.to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Hex-encode a string so it is safe inside a URL path segment and cannot
    /// collide with the separator used by `scoped_id`.
    fn hex_encode(value: &str) -> String {
        let mut out = String::with_capacity(value.len() * 2);
        for byte in value.as_bytes() {
            out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
            out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
        }
        out
    }

    fn scoped_id(&self, doc_id: &str) -> String {
        let prefix = Self::hex_encode(&self.logical_index);
        if self.config.single_index {
            format!("{prefix}_{}", Self::hex_encode(doc_id))
        } else {
            Self::hex_encode(doc_id)
        }
    }

    fn fence_id(&self) -> String {
        let prefix = Self::hex_encode(&self.logical_index);
        format!("{prefix}_{FENCE_DOC_KEY}")
    }

    /// Effective ES index URL for this engine instance.
    fn index_url(&self) -> String {
        if self.config.single_index {
            format!("{}/{}", self.config.url, self.base_index)
        } else {
            format!(
                "{}/{}_{}",
                self.config.url,
                self.base_index,
                Self::sanitize_index_name(&self.logical_index)
            )
        }
    }

    /// Non-scoring filters that isolate this logical index and exclude the
    /// internal fence document from every read path.
    fn scope_filters(&self) -> Vec<JsonValue> {
        let mut filters = vec![json!({ "term": { "doc_type": DOC_TYPE_DOCUMENT } })];
        if self.config.single_index {
            filters.push(json!({ "term": { "logical_index": self.logical_index } }));
        }
        filters
    }

    fn scoped_query(&self, inner: JsonValue) -> JsonValue {
        json!({ "bool": { "must": [inner], "filter": self.scope_filters() } })
    }

    /// Explicit index mapping so keyword fields stay keyword (dynamic mapping
    /// would otherwise analyze them as text). An existing index is accepted.
    pub async fn ensure_index(&self) -> Result<(), SearchError> {
        let mapping = json!({
            "mappings": {
                "properties": {
                    "logical_index": { "type": "keyword" },
                    "doc_id": { "type": "keyword" },
                    "doc_type": { "type": "keyword" },
                    "content": { "type": "text" },
                    "payload": { "type": "text", "index": false }
                }
            }
        });
        let url = self.index_url();
        let result = self
            .request(Method::Put, url, RequestBody::Json(mapping))
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(SearchError::ConfigError(ref msg))
                if msg.contains("resource_already_exists_exception") =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn build_request(&self, method: Method, url: &str, body: RequestBody) -> RequestBuilder {
        let builder = match method {
            Method::Get => self.http.get(url),
            Method::Post => self.http.post(url),
            Method::Put => self.http.put(url),
        };
        let builder = match (&self.config.username, &self.config.password) {
            (Some(user), Some(pass)) => builder.basic_auth(user, Some(pass)),
            _ => builder,
        };
        match body {
            RequestBody::None => builder,
            RequestBody::Json(value) => builder.json(&value),
            RequestBody::Ndjson(text) => {
                builder.header("Content-Type", "application/x-ndjson").body(text)
            }
        }
    }

    fn classify_status(status: StatusCode, body: &str, op: &str) -> SearchError {
        if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
        {
            let _ = body;
            SearchError::EngineUnavailable
        } else {
            SearchError::ConfigError(format!("elasticsearch {op} rejected with {status}: {body}"))
        }
    }

    fn is_transient(err: &SearchError) -> bool {
        matches!(err, SearchError::EngineUnavailable)
    }

    /// Single transport entry point: circuit-breaker guard, auth, retries for
    /// transient failures, and status classification.
    async fn request(
        &self,
        method: Method,
        url: String,
        body: RequestBody,
    ) -> Result<(StatusCode, String), SearchError> {
        self.breaker.lock().check()?;
        let mut attempt = 0u32;
        loop {
            let builder = self.build_request(method, &url, body.clone());
            match builder.send().await {
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() || status == StatusCode::NOT_FOUND {
                        self.breaker.lock().record_success();
                        return Ok((status, text));
                    }
                    let err = Self::classify_status(status, &text, "request");
                    if Self::is_transient(&err) {
                        self.breaker.lock().record_failure();
                        if attempt < self.config.max_retries {
                            attempt += 1;
                            tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                            self.breaker.lock().check()?;
                            continue;
                        }
                    }
                    return Err(err);
                }
                Err(err) => {
                    tracing::warn!(error = %err, url = %url, "elasticsearch transport error");
                    self.breaker.lock().record_failure();
                    if attempt < self.config.max_retries {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                        self.breaker.lock().check()?;
                        continue;
                    }
                    return Err(SearchError::EngineUnavailable);
                }
            }
        }
    }

    /// NDJSON bulk body pairing one `index` action with one source line per
    /// document. Pure function for offline testing.
    fn bulk_index_body(&self, docs: &[(String, String)]) -> String {
        let mut out = String::new();
        for (doc_id, content) in docs {
            let action = json!({ "index": { "_id": self.scoped_id(doc_id) } });
            let source = json!({
                "logical_index": self.logical_index,
                "doc_id": doc_id,
                "doc_type": DOC_TYPE_DOCUMENT,
                "content": content,
            });
            out.push_str(&action.to_string());
            out.push('\n');
            out.push_str(&source.to_string());
            out.push('\n');
        }
        out
    }

    /// NDJSON bulk body of `delete` actions (no source lines).
    fn bulk_delete_body(&self, doc_ids: &[&str]) -> String {
        let mut out = String::new();
        for doc_id in doc_ids {
            let action = json!({ "delete": { "_id": self.scoped_id(doc_id) } });
            out.push_str(&action.to_string());
            out.push('\n');
        }
        out
    }

    /// Search DSL for a plain query string. Pure function for offline testing.
    fn search_body(&self, query: &str, limit: usize) -> JsonValue {
        let inner = if query.trim().is_empty() {
            json!({ "match_all": {} })
        } else {
            json!({ "match": { "content": query } })
        };
        json!({
            "query": self.scoped_query(inner),
            "size": limit,
            "track_scores": true,
            "_source": ["doc_id"],
        })
    }

    /// Translate a structured query into native ES DSL. Phrase/fuzzy/prefix
    /// semantics are preserved instead of degrading to a grammar string.
    fn structured_query(&self, query: &FulltextQuery) -> JsonValue {
        match query {
            FulltextQuery::Simple(text) => json!({ "match": { "content": text } }),
            FulltextQuery::Phrase(text) => json!({ "match_phrase": { "content": text } }),
            FulltextQuery::Prefix(text) => json!({ "prefix": { "content": text } }),
            FulltextQuery::Fuzzy(text, distance) => {
                let fuzziness = match distance {
                    Some(d) => json!(d),
                    None => json!("auto"),
                };
                json!({ "fuzzy": { "content": { "value": text, "fuzziness": fuzziness } } })
            }
            FulltextQuery::Wildcard(text) => json!({ "wildcard": { "content": text } }),
            FulltextQuery::Boolean {
                must,
                should,
                must_not,
            } => {
                let mut clause = serde_json::Map::new();
                if !must.is_empty() {
                    clause.insert(
                        "must".to_string(),
                        json!(must.iter().map(|q| self.structured_query(q)).collect::<Vec<_>>()),
                    );
                }
                if !should.is_empty() {
                    clause.insert(
                        "should".to_string(),
                        json!(should.iter().map(|q| self.structured_query(q)).collect::<Vec<_>>()),
                    );
                    clause.insert("minimum_should_match".to_string(), json!(1));
                }
                if !must_not.is_empty() {
                    clause.insert(
                        "must_not".to_string(),
                        json!(must_not.iter().map(|q| self.structured_query(q)).collect::<Vec<_>>()),
                    );
                }
                json!({ "bool": clause })
            }
            FulltextQuery::Range {
                lower,
                upper,
                include_lower,
                include_upper,
            } => {
                let mut range = serde_json::Map::new();
                if let Some(value) = lower {
                    if *include_lower {
                        range.insert("gte".to_string(), json!(value));
                    } else {
                        range.insert("gt".to_string(), json!(value));
                    }
                }
                if let Some(value) = upper {
                    if *include_upper {
                        range.insert("lte".to_string(), json!(value));
                    } else {
                        range.insert("lt".to_string(), json!(value));
                    }
                }
                json!({ "range": { "content": range } })
            }
        }
    }

    fn parse_hits(payload: &JsonValue) -> Vec<SearchResult> {
        let empty = Vec::new();
        let entries = payload
            .pointer("/hits/hits")
            .and_then(JsonValue::as_array)
            .unwrap_or(&empty);
        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let score = entry.get("_score").and_then(JsonValue::as_f64).unwrap_or(0.0) as f32;
            let doc_id = entry
                .pointer("/_source/doc_id")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            results.push(SearchResult::new(Value::string(doc_id), score));
        }
        results
    }

    /// Refresh pending writes so they become visible to search and count.
    async fn refresh(&self) -> Result<(), SearchError> {
        let url = format!("{}/_refresh", self.index_url());
        self.request(Method::Post, url, RequestBody::None)
            .await
            .map(|_| ())
    }

    async fn bulk(&self, url_suffix: &str, body: String) -> Result<(), SearchError> {
        if body.is_empty() {
            return Ok(());
        }
        let url = format!("{}/{url_suffix}", self.index_url());
        let (_, text) = self
            .request(Method::Post, url, RequestBody::Ndjson(body))
            .await?;
        let payload: JsonValue = serde_json::from_str(&text)
            .map_err(|e| SearchError::SerializationError(format!("es bulk parse failed: {e}")))?;
        if payload.get("errors").and_then(JsonValue::as_bool).unwrap_or(false) {
            return Err(SearchError::EngineUnavailable);
        }
        Ok(())
    }

    /// Read back the commit fence written by `commit_with_payload`.
    ///
    /// The trait exposes only a synchronous `commit_payload` accessor, which
    /// cannot perform ES I/O and therefore returns `None`. This async variant
    /// is the real fence read for remote backends; consistency does not depend
    /// on it because ES writes are idempotent by `_id` and the sync outbox
    /// carries the durable LSN. It is provided for diagnostics and future
    /// online-rebuild catch-up.
    #[allow(dead_code)]
    pub async fn read_commit_payload(&self) -> Result<Option<String>, SearchError> {
        let url = format!(
            "{}/_doc/{}",
            self.index_url(),
            Self::hex_encode(&self.fence_id())
        );
        let (status, text) = self.request(Method::Get, url, RequestBody::None).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let payload: JsonValue = serde_json::from_str(&text)
            .map_err(|e| SearchError::SerializationError(format!("es fence parse failed: {e}")))?;
        Ok(payload
            .pointer("/_source/payload")
            .and_then(JsonValue::as_str)
            .map(str::to_string))
    }
}

#[async_trait]
impl FulltextSearchEngine for EsSearchEngine {
    fn name(&self) -> &str {
        "elasticsearch"
    }

    fn version(&self) -> &str {
        "es-http-1"
    }

    async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError> {
        self.index_batch(vec![(doc_id.to_string(), content.to_string())])
            .await
    }

    async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError> {
        if docs.is_empty() {
            return Ok(());
        }
        let mut handled = 0usize;
        for chunk in docs.chunks(self.config.bulk_size.max(1)) {
            let body = self.bulk_index_body(chunk);
            self.bulk("_bulk", body).await?;
            handled += chunk.len();
        }
        let _ = handled;
        Ok(())
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        let body = self.search_body(query, limit);
        let url = format!("{}/_search", self.index_url());
        let (_, text) = self.request(Method::Post, url, RequestBody::Json(body)).await?;
        let payload: JsonValue = serde_json::from_str(&text)
            .map_err(|e| SearchError::SerializationError(format!("es search parse failed: {e}")))?;
        Ok(Self::parse_hits(&payload))
    }

    async fn search_structured(
        &self,
        query: &FulltextQuery,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let body = json!({
            "query": self.scoped_query(self.structured_query(query)),
            "size": limit,
            "track_scores": true,
            "_source": ["doc_id"],
        });
        let url = format!("{}/_search", self.index_url());
        let (_, text) = self.request(Method::Post, url, RequestBody::Json(body)).await?;
        let payload: JsonValue = serde_json::from_str(&text)
            .map_err(|e| SearchError::SerializationError(format!("es search parse failed: {e}")))?;
        Ok(Self::parse_hits(&payload))
    }

    async fn delete(&self, doc_id: &str) -> Result<(), SearchError> {
        self.delete_batch(vec![doc_id]).await
    }

    async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError> {
        if doc_ids.is_empty() {
            return Ok(());
        }
        let body = self.bulk_delete_body(&doc_ids);
        self.bulk("_bulk", body).await
    }

    async fn ensure_ready(&self) -> Result<(), SearchError> {
        self.ensure_index().await
    }

    async fn commit(&self) -> Result<(), SearchError> {
        self.refresh().await
    }

    async fn commit_with_payload(&self, payload: String) -> Result<(), SearchError> {
        // Refresh data writes first, then persist the fence, then refresh the
        // fence. A read-back payload therefore always refers to visible data.
        self.refresh().await?;
        let url = format!(
            "{}/_doc/{}",
            self.index_url(),
            Self::hex_encode(&self.fence_id())
        );
        let body = json!({ "doc_type": DOC_TYPE_FENCE, "payload": payload });
        self.request(Method::Put, url, RequestBody::Json(body))
            .await?;
        self.refresh().await
    }

    fn commit_payload(&self) -> Result<Option<String>, SearchError> {
        // ES reads require async I/O; the fence is recovered via the async
        // `read_commit_payload` on `EsSearchEngine`. Consistency does not rely
        // on this sync accessor: ES writes are idempotent by `_id` and the
        // sync outbox carries the durable LSN.
        Ok(None)
    }

    async fn stats(&self) -> Result<IndexStats, SearchError> {
        let body = json!({ "query": { "bool": { "filter": self.scope_filters() } } });
        let url = format!("{}/_count", self.index_url());
        let (_, text) = self.request(Method::Post, url, RequestBody::Json(body)).await?;
        let payload: JsonValue = serde_json::from_str(&text)
            .map_err(|e| SearchError::SerializationError(format!("es count parse failed: {e}")))?;
        let count = payload.get("count").and_then(JsonValue::as_u64).unwrap_or(0) as usize;
        Ok(IndexStats::new(count, 0))
    }

    fn consistency_state(&self) -> ConsistencyState {
        match self.consistency.load(Ordering::Acquire) {
            STATE_INCONSISTENT => ConsistencyState::Inconsistent,
            STATE_REBUILDING => ConsistencyState::Rebuilding,
            _ => ConsistencyState::Consistent,
        }
    }

    fn mark_inconsistent(&self) {
        self.consistency.store(STATE_INCONSISTENT, Ordering::Release);
    }

    fn mark_consistent(&self) {
        self.consistency.store(STATE_CONSISTENT, Ordering::Release);
    }

    async fn clear(&self) -> Result<(), SearchError> {
        let body = json!({ "query": { "bool": { "filter": self.scope_filters() } } });
        let url = format!("{}/_delete_by_query", self.index_url());
        self.request(Method::Post, url, RequestBody::Json(body))
            .await
            .map(|_| ())
    }

    async fn close(&self) -> Result<(), SearchError> {
        self.consistency.store(STATE_CONSISTENT, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as J;

    fn engine(single_index: bool) -> EsSearchEngine {
        EsSearchEngine::new(
            EsConfig {
                url: "http://127.0.0.1:9200".to_string(),
                index_name: "LinkRS_Fulltext".to_string(),
                single_index,
                ..Default::default()
            },
            "space_ft_1_person_name".to_string(),
        )
        .expect("test engine builds without I/O")
    }

    #[test]
    fn index_name_is_sanitized() {
        assert_eq!(EsSearchEngine::sanitize_index_name("LinkRS.Fulltext"), "linkrs_fulltext");
    }

    #[test]
    fn scoped_id_is_stable_and_separated() {
        let e = engine(true);
        let a = e.scoped_id("entity:1");
        let b = e.scoped_id("entity:2");
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '_'));
        let single = engine(false);
        assert_ne!(e.scoped_id("doc"), single.scoped_id("doc"));
    }

    #[test]
    fn bulk_body_pairs_action_and_source() {
        let e = engine(true);
        let body = e.bulk_index_body(&[("d1".to_string(), "hello world".to_string())]);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let action: J = serde_json::from_str(lines[0]).expect("action is json");
        let source: J = serde_json::from_str(lines[1]).expect("source is json");
        assert_eq!(action.pointer("/index/_id").and_then(J::as_str).unwrap(), e.scoped_id("d1"));
        assert_eq!(source.get("doc_type").and_then(J::as_str), Some(DOC_TYPE_DOCUMENT));
        assert_eq!(source.get("content").and_then(J::as_str), Some("hello world"));
    }

    #[test]
    fn bulk_delete_body_has_no_source_lines() {
        let e = engine(true);
        let body = e.bulk_delete_body(&["d1", "d2"]);
        assert_eq!(body.lines().count(), 2);
        for line in body.lines() {
            let action: J = serde_json::from_str(line).expect("action is json");
            assert!(action.get("delete").is_some());
        }
    }

    #[test]
    fn search_body_scopes_and_excludes_fence() {
        let e = engine(true);
        let body = e.search_body("rust es", 7);
        assert_eq!(body.get("size").and_then(J::as_u64), Some(7));
        let filters = body.pointer("/query/bool/filter").and_then(J::as_array).expect("filters");
        let text = serde_json::to_string(filters).expect("filters serialize");
        assert!(text.contains(DOC_TYPE_DOCUMENT));
        assert!(text.contains("space_ft_1_person_name"));
        assert!(!text.contains(DOC_TYPE_FENCE));
    }

    #[test]
    fn empty_query_degrades_to_match_all() {
        let e = engine(true);
        let body = e.search_body("   ", 5);
        assert!(body.to_string().contains("match_all"));
    }

    #[test]
    fn structured_query_preserves_phrase_and_boolean() {
        let e = engine(true);
        let phrase = e.structured_query(&FulltextQuery::Phrase("exact words".to_string()));
        assert!(phrase.get("match_phrase").is_some());
        let boolean = e.structured_query(&FulltextQuery::Boolean {
            must: vec![FulltextQuery::Simple("a".to_string())],
            should: vec![FulltextQuery::Prefix("b".to_string())],
            must_not: vec![FulltextQuery::Fuzzy("c".to_string(), Some(1))],
        });
        let b = boolean.get("bool").expect("bool clause");
        assert!(b.get("must").is_some());
        assert!(b.get("should").is_some());
        assert_eq!(b.get("minimum_should_match").and_then(J::as_u64), Some(1));
        assert!(b.get("must_not").is_some());
    }

    #[test]
    fn status_classification_separates_transient_from_config() {
        assert!(matches!(
            EsSearchEngine::classify_status(StatusCode::INTERNAL_SERVER_ERROR, "", "x"),
            SearchError::EngineUnavailable
        ));
        assert!(matches!(
            EsSearchEngine::classify_status(StatusCode::TOO_MANY_REQUESTS, "", "x"),
            SearchError::EngineUnavailable
        ));
        assert!(matches!(
            EsSearchEngine::classify_status(StatusCode::BAD_REQUEST, "bad", "x"),
            SearchError::ConfigError(_)
        ));
    }

    #[test]
    fn breaker_opens_after_threshold_and_blocks() {
        let mut b = EsBreaker::new();
        for _ in 0..EsBreaker::THRESHOLD {
            b.record_failure();
        }
        assert!(b.check().is_err());
        b.record_success();
        assert!(b.check().is_ok());
    }

    #[test]
    fn parse_hits_reads_doc_id_and_score() {
        let payload = json!({ "hits": { "hits": [
            { "_score": 1.5, "_source": { "doc_id": "v42" } }
        ]}});
        let results = EsSearchEngine::parse_hits(&payload);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, 1.5);
        assert_eq!(results[0].doc_id.to_string().unwrap_or_default(), "v42");
    }

    #[test]
    fn consistency_state_transitions() {
        let e = engine(true);
        assert_eq!(e.consistency_state(), ConsistencyState::Consistent);
        e.mark_inconsistent();
        assert_eq!(e.consistency_state(), ConsistencyState::Inconsistent);
        e.mark_consistent();
        assert_eq!(e.consistency_state(), ConsistencyState::Consistent);
    }
}
