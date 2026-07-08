use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TantivyConfig {
    pub writer_memory_budget: usize,
    #[serde(default)]
    pub tokenizer: TokenizerKind,
    #[serde(default = "default_doc_store_cache_num_blocks")]
    pub doc_store_cache_num_blocks: usize,
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
pub struct FulltextConfig {
    pub enabled: bool,
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
