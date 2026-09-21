use graphdb_core::types::Timestamp;
use graphdb_core::Value;

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
    /// Row creation stamp observed when the entry was seeded. Hit
    /// revalidation rejects the entry when the live row's creation stamp
    /// drifted (delete/recreate, GC remap) instead of snapshot a stale row.
    pub create_ts: Timestamp,
    /// Per-column covering version stamps observed at seed time. Hit
    /// revalidation compares them against the stamps covering the query
    /// timestamp so a concurrent property write between seed and hit turns
    /// the hit into a miss instead of snapshot the pre-write value.
    pub column_starts: Vec<Timestamp>,
    /// Cache-internal invalidation generation of the owning label, assigned
    /// by `RecordCache` on insert. Not meaningful to storage consumers.
    pub generation: u32,
}

/// Cached ID index value.
#[derive(Debug, Clone, Copy)]
pub struct IdIndexCacheValue {
    pub internal_id: u32,
    /// Snapshot timestamp the mapping was loaded at. Served to readers at
    /// or past this timestamp while no write has invalidated the entry.
    pub cached_at_ts: Timestamp,
    /// Cache-internal invalidation generation of the owning label, assigned
    /// by `RecordCache` on insert.
    pub generation: u32,
}

impl CachedVertex {
    pub fn estimated_size(&self) -> u32 {
        let mut size = std::mem::size_of::<Self>();

        size += self.external_id.capacity();
        size += self.column_starts.len() * std::mem::size_of::<Timestamp>();

        for (name, value) in &self.properties {
            size += name.capacity();
            size += value.estimated_size();
        }

        size as u32
    }
}
