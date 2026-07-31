//! Storage Module
//!
//! Core storage layer for the graph database, providing:
//! - Columnar storage for vertices and edges (CSR)
//! - Index: Primary and secondary indexes
//! - Cache: Record caching
//! - Engine: Storage engine core

pub(crate) mod cache;
pub(crate) mod client;
pub(crate) mod cold;
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

mod metrics;
pub(crate) mod persistence;
pub(crate) mod safe_read;
pub(crate) mod types;
pub(crate) mod vertex;

#[cfg(any(test, feature = "test-support"))]
mod test_mock;

pub use client::{
    CatalogStore, GraphStore, QueryStorage, StorageAdmin, StorageAuthOps, StorageClient,
    StorageCommitOps, StorageGcOps, StorageMaintenance, StorageOperationContext,
    StorageOperationContextOps, StoragePersistenceOps, StorageReader, StorageRecoveryOps,
    StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps, StorageStats,
    StorageSyncContextOps, StorageWriter,
};
pub use cursor::{
    open_edge_scan, open_index_cursor, open_vertex_scan, EdgeCursor, IndexCursor, IndexPredicate,
    IndexRow, IndexScanPlan, PartitionSelector, RequiredProperty, ScanOptions, ScanTarget,
    VecEdgeCursor, VecVertexCursor, VertexCursor,
};
pub use engine::config::{PropertyGraphConfig, ResourceConfig};
pub use engine::graph_storage::GraphStorage;
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

pub use cold::cold_snapshot::ColdSnapshot;
pub use crate::core::StorageError;

#[cfg(any(test, feature = "test-support"))]
pub use test_mock::MockStorage;
