//! Storage Module
//!
//! Core storage layer for the graph database, providing:
//! - Columnar storage for vertices and edges (CSR)
//! - Index: Primary and secondary indexes
//! - Cache: Record caching
//! - Engine: Storage engine core

pub(crate) mod cache;
pub(crate) mod client;
pub(crate) mod compression;
pub(crate) mod edge;
pub(crate) mod encoding;
pub(crate) mod engine;
pub(crate) mod index;
pub mod mvcc;
pub(crate) mod naming;
pub(crate) mod schema;
pub mod sync;

mod metrics;
pub(crate) mod persistence;
pub(crate) mod types;
pub mod vertex;

#[cfg(any(test, feature = "test-support"))]
mod test_mock;

pub use client::{
    StorageAdmin, StorageAuthOps, StorageClient, StorageGcOps, StoragePersistenceOps,
    StorageReader, StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps,
    StorageStats, StorageSyncContextOps, StorageTransactionContextOps, StorageWriter,
};
pub use engine::graph_storage::GraphStorage;
pub use engine::persistence_coordinator::{CheckpointStats, SnapshotStats};
pub use engine::sync_wrapper::SyncWrapper;
pub use engine::transaction::UndoTarget;
pub use metrics::MetricsStorage;
pub use mvcc::{MVCCTable, SnapshotHandle, TieredTombstoneManager, TombstoneEntry};
pub use sync::{EdgeTableSync, PropertyTableSync, SnapshotGuard, VertexTableSync};
pub use types::StoragePropertyDef;
pub use vertex::{VertexSchema, VertexTable};

pub use crate::core::StorageError;

#[cfg(any(test, feature = "test-support"))]
pub use test_mock::MockStorage;
