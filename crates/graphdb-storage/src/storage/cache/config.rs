use std::time::Duration;

/// Configuration for record cache
///
/// Note: `max_memory` and `memory_ratio` are applied at build time via
/// Moka's `Cache::builder().max_capacity(...)`. They cannot be changed
/// at runtime — recreate `RecordCache` to apply new values.
#[derive(Debug, Clone)]
pub struct RecordCacheConfig {
    /// Maximum memory usage in bytes (applied at build time)
    pub max_memory: usize,
    /// Memory distribution ratio: (vertex, id_index)
    /// Applied at build time — runtime changes require recreating RecordCache.
    pub memory_ratio: (u32, u32),
    /// Time-to-live for cache entries
    pub ttl: Option<Duration>,
    /// Time-to-idle for cache entries
    pub tti: Option<Duration>,
    /// Ratio of memory allocated for high-priority entries (id_index).
    /// Applied at build time.
    pub high_priority_ratio: f32,
}

impl Default for RecordCacheConfig {
    fn default() -> Self {
        Self {
            max_memory: 128 * 1024 * 1024,
            memory_ratio: (70, 30),
            ttl: Some(Duration::from_secs(3600)),
            tti: Some(Duration::from_secs(300)),
            high_priority_ratio: 0.1,
        }
    }
}
