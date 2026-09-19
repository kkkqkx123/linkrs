//! Edge table module: node-group sharded CSR with row-level timestamps.
//!
//! Organization:
//! - `core`: EdgeStore table definition plus per-responsibility operation
//!   modules (`core/store`, `core/owner`, `core/reads`, `core/writes`,
//!   `core/index`, `core/query`, `core/schema_ops`, `core/maintenance`,
//!   `core/recovery`)
//! - `checkpoint`: incremental per-group checkpoint (manifest, group files,
//!   timestamp and property shards)
//! - `compaction`: per-group CSR compaction and property cleanup
//! - `iterator`: sharded scan iterator
//! - `mvcc`: centralized timestamps, tombstones, and GC watermarks
//! - `persistence`: version-5 serialization (flush/load)
//! - `remap`: vertex ID remapping plus offline group-width resharding
//! - `schema_add_column`: staged add-column state machine (prepare/fill/publish/abort)
//! - `schema_drop_column`: staged drop-column state machine (prepare/publish/abort)
//! - `schema_rename_column`: staged rename-column state machine (prepare/publish/abort)
//! - `stats`: tombstone and deletion statistics
//! - `freeze`: explicit per-direction group freeze and unfreeze
//!
//! Write batching contract: the engine layer currently commits one edge per
//! staging batch (`insert_edge`/`delete_edge` each wrap a single staged
//! entry in `commit_staging_batch`). A multi-entry `EdgeStagingBatch` is
//! supported by `commit_staging_batch` for future transaction-layer batching,
//! but no engine path builds one today. Each single-entry commit is atomic
//! with concentrated rollback; multi-edge transaction atomicity still relies
//! on the undo-log replay above this layer.

pub mod checkpoint;
pub mod compaction;
pub mod config;
pub mod core;
pub mod freeze;
pub mod iterator;
pub mod mvcc;
pub mod persistence;
pub mod remap;
pub mod schema_add_column;
pub mod schema_drop_column;
pub mod schema_rename_column;
pub mod staging;
pub mod stats;
pub mod wal;

// Re-export commonly used types
pub use core::{EdgeStore, UpdateEdgePropertyByKeyParams};
pub use iterator::{AdjacencyBatchAccessor, EdgeTableScanIterator, DEFAULT_ADJACENCY_BATCH};
pub use staging::{EdgeStagingBatch, StagedDelete, StagedInsert};
pub use stats::{DeletionStats, GroupSegmentStats, ScanPruneReport, TombstoneStats};

// Re-export from parent
pub use super::{CsrBase, CsrVariant, Nbr};
