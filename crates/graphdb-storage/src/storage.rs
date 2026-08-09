//! Storage Module
//!
//! Core storage layer for the graph database, providing:
//! - Columnar storage for vertices and edges (CSR)
//! - Index: Primary and secondary indexes
//! - Cache: Record caching
//! - Engine: Storage engine core

pub(crate) mod cache;
pub(crate) mod client;
pub mod cold;
pub(crate) mod column_stats;
pub(crate) mod compression;
pub(crate) mod cursor;
pub(crate) mod edge;
pub(crate) mod encoding;
pub(crate) mod engine;
pub(crate) mod index;
pub(crate) mod macros;
pub(crate) mod mvcc;
pub(crate) mod naming;
pub(crate) mod schema;
pub(crate) mod thread_pool;

mod batch_ops;
mod metrics;
pub(crate) mod persistence;
pub(crate) mod safe_read;
pub(crate) mod types;
pub(crate) mod vertex;

#[cfg(any(test, feature = "test-support"))]
mod test_mock;

pub use batch_ops::AutoCommitBatchOps;
pub use client::{
    CatalogStore, ColdSnapshotInfo, GraphStore, QueryStorage, StorageAdmin, StorageAuthOps,
    StorageClient, StorageCommitOps, StorageGcOps, StorageMaintenance, StorageOperationContext,
    StorageOperationContextOps, StoragePersistenceOps, StorageReader, StorageRecoveryOps,
    StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps, StorageStats,
    StorageSyncContextOps, StorageWriter,
};
pub use cursor::{
    open_edge_scan, open_index_cursor, open_vertex_scan, ColumnValues, EdgeColumnBatch,
    EdgeCursor, FlatVertexRecord, IndexCursor, IndexPredicate, IndexRow, IndexScanPlan,
    PartitionSelector, PropertyColumn, RequiredProperty, ScanOptions, ScanPredicate, ScanTarget,
    VecEdgeCursor, VecVertexCursor, VertexColumnBatch, VertexCursor,
};
pub use engine::config::{ColdTierConfig, PropertyGraphConfig, ResourceConfig};
pub use engine::graph_storage::{AutoCommitBatchWindow, GraphStorage};
pub use engine::persistence_coordinator::{
    CatalogLockDiagnostic, CheckpointStats, PersistenceConfig, PersistenceDiagnostics,
    PersistenceFaultPoint, SnapshotStats,
};
pub use engine::resource_budget::{
    MemoryAccounting, MemoryBudget, MemoryCategory, MemoryReservation, MemoryUsage,
    ResourceSnapshot,
};
pub use engine::sync_wrapper::SyncWrapper;
pub use engine::transaction::UndoTarget;
pub use engine::WalMetrics;
pub use index::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
    ManifestCatalogStats, ManifestHandle,
};
pub use metrics::MetricsStorage;
pub use schema::{ChangeDetails, ChangeLog, LabelVersionHistory, PropertyChange, SchemaObjectType};
pub use types::StoragePropertyDef;

pub use crate::core::StorageError;
pub use cold::cold_snapshot::ColdSnapshot;

#[cfg(any(test, feature = "test-support"))]
pub use test_mock::MockStorage;
