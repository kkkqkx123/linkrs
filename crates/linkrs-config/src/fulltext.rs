use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Local fulltext engine selector.
///
/// Only the local Tantivy BM25 engine is supported. The enum is kept so
/// stored configuration stays explicit about which engine produced an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FulltextEngineType {
    #[default]
    Bm25,
}

impl std::fmt::Display for FulltextEngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FulltextEngineType::Bm25 => write!(f, "bm25"),
        }
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
    /// Engine that produced the indexes. Only the local BM25 engine exists;
    /// the field documents index provenance and must stay `Bm25`.
    pub default_engine: FulltextEngineType,
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
            default_engine: FulltextEngineType::default(),
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
}
