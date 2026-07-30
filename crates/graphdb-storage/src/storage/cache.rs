//! Cache Module
//!
//! Provides caching mechanisms for the storage engine.
//!
//! ## Cache Types
//!
//! ### Vertex Cache (Default)
//! - Caches vertex records for fast point lookups
//! - Caches external_id -> internal_id mappings
//!
//! ### Buffer Pool
//! - Generic clock-algorithm cache with pin/unpin support
//! - Used by ChunkedIndex for index chunk caching

mod buffer_pool;
mod config;
mod record_cache;
mod types;

#[cfg(test)]
mod record_cache_test;

pub(crate) use buffer_pool::BufferPool;
pub use config::RecordCacheConfig;
pub use record_cache::{RecordCache, RecordCacheStats, SharedRecordCache};
pub use types::{CachedVertex, EvictionCallbackWithSize, VertexCacheKey};
