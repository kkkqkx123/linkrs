pub mod config;
pub mod engine;
pub mod error;
#[cfg(feature = "jieba")]
pub mod jieba_tokenizer;
#[cfg(feature = "fulltext-search")]
pub mod manager;
pub mod metadata;
#[cfg(feature = "fulltext-search")]
pub mod metrics;
pub mod result;
#[cfg(feature = "fulltext-search")]
pub mod tantivy_index;
#[cfg(feature = "fulltext-search")]
pub mod warmup;

#[cfg(test)]
mod isolation_test;

pub use crate::config::common::fulltext::{TantivyConfig, TokenizerKind};
pub use config::{FulltextConfig, SyncConfig, SyncFailurePolicy};
pub use engine::{ConsistencyState, EngineType};
pub use error::{Result, SearchError};
#[cfg(feature = "fulltext-search")]
pub use manager::FulltextIndexManager;
pub use metadata::{IndexKey, IndexMetadata, IndexStatus};
#[cfg(feature = "fulltext-search")]
pub use metrics::MetricsSearchEngine;
pub use result::{
    FulltextSearchEntry, FulltextSearchResult, HighlightResult, IndexStats, SearchResult,
    SearchStats,
};
#[cfg(feature = "fulltext-search")]
pub use tantivy_index::TantivySearchEngine;
#[cfg(feature = "fulltext-search")]
pub use warmup::IndexWarmer;
