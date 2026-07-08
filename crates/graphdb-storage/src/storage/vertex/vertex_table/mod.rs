//! VertexTable module organization
//!
//! Split into logical components:
//! - core: CRUD operations and queries
//! - persistence: File I/O, serialization
//! - optimizer: Compaction and optimization
//! - schema: Schema management
//! - compaction: Unified compaction coordinator (medium-term improvement)

pub mod core;
pub mod optimizer;
pub mod persistence;
pub mod schema;
pub mod compaction;

pub use core::{VertexTable, VertexTableConfig, VertexIterator};
pub use compaction::CompactionCoordinator;
