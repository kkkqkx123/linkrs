//! Index Module
//!
//! Provide index data management functions, including index data update, delete and query.
//! Note: Index metadata management is the responsibility of the metadata::IndexMetadataManager.
//!
//! ## Property Indexes
//!
//! BTreeMap-based property indexes supporting complex queries with MVCC:
//! - `vertex_index_manager`: Index on vertex properties
//!
//! Characteristics:
//! - Support MVCC for snapshot isolation
//! - BTreeMap-based for range queries
//! - Support tombstone GC for deleted entries
//! - Optional key compression for memory efficiency
//!
//! ## Module Structure
//!
//! - `vertex_index_manager`: BTreeMap-based vertex index management
//! - `index_data_manager`: `IndexDataManagerImpl` with `VertexIndexOps`, `IndexGcOps`
//! - `key_codec`: Index key encoding/decoding and compression utilities
//! - `index_gc_manager`: Background GC for tombstone cleanup

pub(crate) mod generic_index_manager;
pub(crate) mod index_data_manager;
pub(crate) mod index_gc_manager;
pub(crate) mod key_codec;
pub(crate) mod vertex_index_manager;

pub use index_data_manager::{GcStats, IndexDataManagerImpl, IndexGcOps, VertexIndexOps};
pub use index_gc_manager::{IndexGcConfig, IndexGcManager};
