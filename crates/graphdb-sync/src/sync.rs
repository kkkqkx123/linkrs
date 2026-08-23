#[cfg(feature = "vector")]
pub mod backend;
pub mod batch;
pub mod builder;
pub mod checkpoint_manifest;
pub mod circuit_breaker;
#[cfg(feature = "fulltext-search")]
pub mod coordinator;
pub mod dead_letter_queue;
pub mod manager;
mod outbox;
pub mod outbox_recovery;
pub mod receiver;
pub mod retry;
pub mod sqlite_outbox;
pub mod types;
pub mod vector_error;
#[cfg(feature = "vector")]
pub mod vector_sync;

pub use crate::search::SyncConfig;
#[cfg(feature = "fulltext-search")]
pub use batch::FulltextBatchProcessor;
pub use batch::{BatchConfig, BatchError, BatchProcessor, TransactionBatchBuffer};
pub use checkpoint_manifest::{
    CheckpointManifest, CheckpointManifestManager, IndexManifestRef, OutboxSnapshotRef,
    StorageFileRef, StorageSnapshotRef,
};
pub use circuit_breaker::{
    with_circuit_breaker, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
    CircuitBreakerStats, CircuitState,
};
#[cfg(feature = "fulltext-search")]
pub use coordinator::{
    ChangeContext, ChangeData, ChangeType, IndexType, RecoveryResult, SyncCoordinator,
    SyncCoordinatorError,
};
pub use dead_letter_queue::{DeadLetterEntry, DeadLetterQueue, DeadLetterQueueConfig};
pub use manager::{EdgeProps, EdgeRef, OutboxConsumerConfig, SyncError, SyncManager};
pub use outbox::{OutboxPayload, OutboxStats};
pub use outbox_recovery::{
    find_latest_snapshot, find_latest_snapshot_at_or_before, live_database_exists, recover_outbox,
    restore_latest_snapshot, restore_snapshot_sync, verify_live_database,
};
#[cfg(feature = "fulltext-search")]
pub use receiver::FulltextReceiver;
#[cfg(feature = "vector-qdrant")]
pub use receiver::VectorReceiver;
pub use receiver::{ApplyReceipt, LateArrivalResult};
pub use retry::{with_retry, RetryConfig};
pub use sqlite_outbox::{
    ClaimedEvent, IndexSyncDiagnostics, OutboxSnapshot, SqliteOutbox, SyncDiagnostics,
    TargetSyncDiagnostics,
};
pub use types::{IndexOpKey, IndexOperation};

#[cfg(feature = "vector")]
pub use vector_sync::{
    PendingVectorUpdate, VectorChangeContext, VectorChangeType, VectorEngineState,
    VectorIndexLocation, VectorPointData, VectorSyncCoordinator, VectorTransactionBuffer,
    VectorTransactionBufferConfig,
};

#[cfg(feature = "vector")]
pub use backend::VectorBackend;
#[cfg(feature = "vector")]
pub use vector_search::HealthStatus;

// Re-export the remote client surface so downstream crates (root integration
// tests, embedded API) can reference it without a direct vector-client
// dependency. Only available with the qdrant feature.
#[cfg(feature = "vector-qdrant")]
pub use vector_client::{VectorClientConfig, VectorManager};
