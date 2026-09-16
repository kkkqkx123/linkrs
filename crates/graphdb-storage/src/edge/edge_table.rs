//! Edge table module: node-group sharded CSR with row-level timestamps.
//!
//! Organization:
//! - `core`: EdgeStore operations (CRUD, properties, queries, persistence)
//! - `checkpoint`: incremental per-group checkpoint (manifest, group files)
//! - `compaction`: per-group CSR compaction and property cleanup
//! - `iterator`: sharded scan iterator
//! - `mvcc`: centralized timestamps, tombstones, and GC watermarks
//! - `persistence`: version-2 serialization (flush/load)
//! - `remap`: vertex ID remapping across node groups
//! - `stats`: tombstone and deletion statistics

pub mod checkpoint;
pub mod compaction;
pub mod config;
pub mod core;
pub mod iterator;
pub mod mvcc;
pub mod persistence;
pub mod remap;
pub mod staging;
pub mod stats;

// Re-export commonly used types
pub use core::{EdgeStore, UpdateEdgePropertyByOffsetParams};
pub use staging::{EdgeStagingBatch, StagedDelete, StagedInsert};
pub use stats::{DeletionStats, TombstoneStats};

// Re-export from parent
pub use super::{CsrBase, CsrVariant, Nbr};
