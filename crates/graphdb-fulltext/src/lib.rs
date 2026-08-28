pub mod engine;
pub mod error;
#[cfg(feature = "jieba")]
pub mod jieba_tokenizer;
#[cfg(feature = "fulltext")]
pub mod manager;
pub mod metadata;
#[cfg(feature = "fulltext")]
pub mod metrics;
pub mod result;
#[cfg(feature = "fulltext")]
pub mod tantivy_index;
#[cfg(feature = "fulltext")]
pub mod warmup;

#[cfg(test)]
mod isolation_test;

pub use graphdb_config::fulltext::{
    Bm25Params, FulltextConfig, FulltextEngineType as EngineType, SyncConfig, SyncFailurePolicy,
    TantivyConfig, TokenizerKind,
};
pub use engine::FulltextSearchEngine;
pub use error::{Result, SearchError};
pub use graphdb_core::ConsistencyState;
#[cfg(feature = "fulltext")]
pub use manager::FulltextIndexManager;
pub use metadata::{IndexKey, IndexMetadata, IndexStatus};
#[cfg(feature = "fulltext")]
pub use metrics::MetricsSearchEngine;
pub use result::{
    FulltextSearchEntry, FulltextSearchResult, HighlightResult, IndexStats, SearchResult,
    SearchStats,
};
#[cfg(feature = "fulltext")]
pub use tantivy_index::TantivySearchEngine;
#[cfg(feature = "fulltext")]
pub use warmup::IndexWarmer;
