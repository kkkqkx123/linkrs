use std::sync::Arc;

/// Fulltext/vector index lifecycle notification.
///
/// Emission is best-effort and synchronous on the index management path.
/// Observers must return quickly and must not call back into the index
/// manager while handling an event.
#[derive(Debug, Clone)]
pub enum IndexEvent {
    FulltextBuildStarted {
        index_name: String,
    },
    FulltextBuildCompleted {
        index_name: String,
        docs_count: u64,
    },
    FulltextRefresh {
        index_name: String,
    },
    FulltextDropped {
        index_name: String,
    },
    VectorBuildStarted {
        index_name: String,
    },
    VectorBuildCompleted {
        index_name: String,
        vectors_count: u64,
    },
    VectorDropped {
        index_name: String,
    },
    IndexMergeStarted {
        index_name: String,
        segments: usize,
    },
    IndexMergeCompleted {
        index_name: String,
    },
}

/// Runtime observer for index lifecycle events.
pub type IndexEventCallback = Arc<dyn Fn(&IndexEvent) + Send + Sync>;
