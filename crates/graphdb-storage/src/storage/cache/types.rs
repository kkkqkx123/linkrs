use std::sync::Arc;

use crate::core::types::Timestamp;
use crate::core::Value;

/// Eviction cause for cache entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionCause {
    /// Entry was evicted due to capacity constraints
    Capacity,
    /// Entry expired due to TTL or TTI
    Expired,
    /// Entry was explicitly removed
    Explicit,
    /// Entry was replaced by a new value
    Replaced,
}

/// Callback type for eviction notifications.
pub type EvictionCallback = Arc<dyn Fn(&str, EvictionCause) + Send + Sync>;

/// Memory-aware eviction callback that receives the byte size of the evicted entry.
///
/// # Arguments
/// - `cache_type`: "vertex" or "id_index"
/// - `cause`: why the entry was evicted
/// - `bytes`: estimated byte size of the evicted entry
pub type EvictionCallbackWithSize = Arc<dyn Fn(&str, EvictionCause, u64) + Send + Sync>;

/// Key for vertex cache: (label_id, internal_id)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VertexCacheKey {
    pub label_id: u32,
    pub internal_id: u32,
}

impl VertexCacheKey {
    pub fn new(label_id: u32, internal_id: u32) -> Self {
        Self {
            label_id,
            internal_id,
        }
    }
}

/// Key for ID index cache: (label_id, external_id)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdIndexCacheKey {
    pub label_id: u32,
    pub external_id: String,
}

impl IdIndexCacheKey {
    pub fn new(label_id: u32, external_id: String) -> Self {
        Self {
            label_id,
            external_id,
        }
    }
}

/// Cached vertex record
#[derive(Debug, Clone)]
pub struct CachedVertex {
    pub internal_id: u32,
    pub external_id: String,
    pub properties: Vec<(String, Value)>,
    pub cached_at_ts: Timestamp,
}

/// Cached ID index value.
#[derive(Debug, Clone, Copy)]
pub struct IdIndexCacheValue {
    pub internal_id: u32,
}

impl CachedVertex {
    pub fn estimated_size(&self) -> u32 {
        let mut size = std::mem::size_of::<Self>();

        size += self.external_id.capacity();

        for (name, value) in &self.properties {
            size += name.capacity();
            size += value.estimated_size();
        }

        size as u32
    }
}
