use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where fulltext indexes are physically stored.
///
/// `Local` keeps the embedded Tantivy engine (the historical default).
/// `Es` routes indexing and search through a remote Elasticsearch cluster.
/// The two backends are mutually exclusive for a running instance: switching
/// requires a rebuild, because index data is not portable between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FulltextBackendKind {
    #[default]
    Local,
    Es,
}

impl std::fmt::Display for FulltextBackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FulltextBackendKind::Local => write!(f, "local"),
            FulltextBackendKind::Es => write!(f, "es"),
        }
    }
}

fn default_es_bulk_size() -> usize {
    500
}

fn default_es_request_timeout_ms() -> u64 {
    5_000
}

fn default_es_index_name() -> String {
    "linkrs_fulltext".to_string()
}

/// Connection and behavior settings for the Elasticsearch remote backend.
///
/// Only read when [`FulltextConfig::backend`] is [`FulltextBackendKind::Es`].
/// Authentication is optional: `api_key` (sent as an `ApiKey` header) takes
/// precedence over the `username`/`password` basic-auth pair when both are
/// present.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsConfig {
    /// Cluster base URL (e.g. `http://127.0.0.1:9200`), without trailing slash.
    pub url: String,
    /// Target index name. Shared by every local fulltext index when `single_index` is true.
    #[serde(default = "default_es_index_name")]
    pub index_name: String,
    /// Store every logical fulltext index in one ES index, scoped by a keyword field.
    #[serde(default = "default_true")]
    pub single_index: bool,
    /// API key for `ApiKey` authorization, when set.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Basic-auth user name, when set together with `password`.
    #[serde(default)]
    pub username: Option<String>,
    /// Basic-auth password.
    #[serde(default)]
    pub password: Option<String>,
    /// Maximum number of documents per bulk request.
    #[serde(default = "default_es_bulk_size")]
    pub bulk_size: usize,
    /// Per-request timeout in milliseconds.
    #[serde(default = "default_es_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Number of times a transient remote failure is retried before giving up.
    #[serde(default)]
    pub max_retries: u32,
}

fn default_true() -> bool {
    true
}

impl Default for EsConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            index_name: default_es_index_name(),
            single_index: true,
            api_key: None,
            username: None,
            password: None,
            bulk_size: default_es_bulk_size(),
            request_timeout_ms: default_es_request_timeout_ms(),
            max_retries: 0,
        }
    }
}

impl EsConfig {
    /// Validate remote settings. Empty URLs are accepted while the backend is
    /// disabled so a default config stays loadable; they are rejected when the
    /// ES backend is selected (checked by [`FulltextConfig::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("fulltext.es.url must not be empty".to_string());
        }
        if self.index_name.trim().is_empty() {
            return Err("fulltext.es.index_name must not be empty".to_string());
        }
        if self.bulk_size == 0 {
            return Err("fulltext.es.bulk_size must be greater than 0".to_string());
        }
        if self.request_timeout_ms == 0 {
            return Err("fulltext.es.request_timeout_ms must be greater than 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerKind {
    Jieba,
    Raw,
    #[default]
    Default,
    Whitespace,
}

impl TokenizerKind {
    pub fn name(&self) -> &'static str {
        match self {
            TokenizerKind::Jieba => "jieba",
            TokenizerKind::Raw => "raw",
            TokenizerKind::Default => "default",
            TokenizerKind::Whitespace => "whitespace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bm25Params {
    pub k1: f32,
    pub b: f32,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TantivyConfig {
    pub writer_memory_budget: usize,
    #[serde(default)]
    pub tokenizer: TokenizerKind,
    #[serde(default = "default_doc_store_cache_num_blocks")]
    pub doc_store_cache_num_blocks: usize,
    #[serde(default)]
    pub bm25_params: Bm25Params,
}

fn default_doc_store_cache_num_blocks() -> usize {
    100
}

impl Default for TantivyConfig {
    fn default() -> Self {
        Self {
            writer_memory_budget: 50_000_000,
            tokenizer: TokenizerKind::default(),
            doc_store_cache_num_blocks: 100,
            bm25_params: Bm25Params::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SyncFailurePolicy {
    #[default]
    FailOpen,
    FailClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncConfig {
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
    #[serde(default = "default_commit_interval_ms")]
    pub commit_interval_ms: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default)]
    pub failure_policy: SyncFailurePolicy,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            queue_size: default_queue_size(),
            commit_interval_ms: default_commit_interval_ms(),
            batch_size: default_batch_size(),
            failure_policy: SyncFailurePolicy::default(),
        }
    }
}

fn default_queue_size() -> usize {
    10000
}

fn default_commit_interval_ms() -> u64 {
    1000
}

fn default_batch_size() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FulltextConfig {
    pub enabled: bool,
    /// Physical storage backend for fulltext indexes.
    #[serde(default)]
    pub backend: FulltextBackendKind,
    /// Remote backend settings; only consulted when `backend` is `Es`.
    #[serde(default)]
    pub es: EsConfig,
    pub index_path: PathBuf,
    pub sync: SyncConfig,
    pub tantivy: TantivyConfig,
    pub cache_size: usize,
    pub max_result_cache: usize,
    pub result_cache_ttl_secs: u64,
}

impl Default for FulltextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: FulltextBackendKind::default(),
            es: EsConfig::default(),
            index_path: PathBuf::from("data/fulltext"),
            sync: SyncConfig::default(),
            tantivy: TantivyConfig::default(),
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        }
    }
}

impl FulltextConfig {
    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.cache_size == 0 {
            return Err("fulltext cache_size must be greater than 0".to_string());
        }
        if self.max_result_cache == 0 {
            return Err("fulltext max_result_cache must be greater than 0".to_string());
        }
        if self.result_cache_ttl_secs == 0 {
            return Err("fulltext result_cache_ttl_secs must be greater than 0".to_string());
        }
        if self.tantivy.writer_memory_budget == 0 {
            return Err("fulltext writer_memory_budget must be greater than 0".to_string());
        }
        if self.tantivy.doc_store_cache_num_blocks == 0 {
            return Err("fulltext doc_store_cache_num_blocks must be greater than 0".to_string());
        }
        if !self.tantivy.bm25_params.k1.is_finite() || self.tantivy.bm25_params.k1 < 0.0 {
            return Err("fulltext bm25_params.k1 must be a non-negative number".to_string());
        }
        if !self.tantivy.bm25_params.b.is_finite()
            || !(0.0..=1.0).contains(&self.tantivy.bm25_params.b)
        {
            return Err("fulltext bm25_params.b must be within 0.0..=1.0".to_string());
        }
        if self.backend == FulltextBackendKind::Es {
            self.es.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fulltext_config_validate() {
        assert!(FulltextConfig::default().validate().is_ok());

        let zero_cache = FulltextConfig {
            cache_size: 0,
            ..Default::default()
        };
        assert!(zero_cache.validate().is_err());

        let bm25 = FulltextConfig {
            tantivy: TantivyConfig {
                bm25_params: Bm25Params { k1: 1.2, b: 1.5 },
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(bm25.validate().is_err());
    }

    #[test]
    fn test_es_backend_validation() {
        // Local backend ignores an empty ES url.
        let local = FulltextConfig::default();
        assert_eq!(local.backend, FulltextBackendKind::Local);
        assert!(local.validate().is_ok());

        // Switching to ES without a url is rejected.
        let es_no_url = FulltextConfig {
            backend: FulltextBackendKind::Es,
            ..Default::default()
        };
        assert!(es_no_url.validate().is_err());

        // A complete ES config is accepted.
        let es_ok = FulltextConfig {
            backend: FulltextBackendKind::Es,
            es: EsConfig {
                url: "http://127.0.0.1:9200".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(es_ok.validate().is_ok());

        // Display strings are stable for config logs.
        assert_eq!(FulltextBackendKind::Local.to_string(), "local");
        assert_eq!(FulltextBackendKind::Es.to_string(), "es");
    }

    #[test]
    fn test_es_config_zero_values_rejected() {
        assert!(EsConfig {
            url: "http://127.0.0.1:9200".to_string(),
            bulk_size: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(EsConfig {
            url: "http://127.0.0.1:9200".to_string(),
            request_timeout_ms: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
