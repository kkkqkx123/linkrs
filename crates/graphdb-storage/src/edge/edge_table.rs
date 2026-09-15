//! Edge table module: single-segment CSR with row-level timestamps.
//!
//! Organization:
//! - `core`: EdgeStore operations (CRUD, properties, queries, persistence)
//! - `compaction`: single-segment CSR compaction and property cleanup
//! - `iterator`: single-segment scan iterator
//! - `mvcc`: centralized timestamps, tombstones, and GC watermarks
//! - `persistence`: version-1 serialization (flush/load)
//! - `remap`: vertex ID remapping for single-segment CSRs
//! - `stats`: tombstone and deletion statistics

pub mod compaction;
pub mod config;
pub mod core;
pub mod iterator;
pub mod mvcc;
pub mod persistence;
pub mod remap;
pub mod stats;

// Re-export commonly used types
pub use core::{EdgeStore, UpdateEdgePropertyByOffsetParams};
pub use stats::{DeletionStats, TombstoneStats};

// Re-export from parent
pub use super::{CsrBase, CsrVariant, Nbr};
