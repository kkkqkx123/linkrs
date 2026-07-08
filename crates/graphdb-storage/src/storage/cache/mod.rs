//! Cache Module
//!
//! Provides caching mechanisms for the storage engine.
//!
//! ## Cache Types
//!
//! ### Vertex Cache (Default)
//! - Caches vertex records for fast point lookups
//! - Caches external_id -> internal_id mappings

mod config;
mod record_cache;
mod types;

#[cfg(test)]
mod record_cache_test;

pub use config::RecordCacheConfig;
pub use record_cache::{RecordCache, SharedRecordCache};
pub use types::{CachedVertex, VertexCacheKey};
