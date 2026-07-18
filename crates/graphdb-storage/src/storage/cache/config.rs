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
            ttl: Some(Duration::from_secs(60)),
            tti: Some(Duration::from_secs(300)),
            high_priority_ratio: 0.1,
        }
    }
}

impl RecordCacheConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_memory == 0 {
            return Err("max_memory must be greater than 0".to_string());
        }
        if self.memory_ratio.0 == 0 && self.memory_ratio.1 == 0 {
            return Err("memory_ratio must contain a positive component".to_string());
        }
        if !self.high_priority_ratio.is_finite() || !(0.0..=1.0).contains(&self.high_priority_ratio)
        {
            return Err("high_priority_ratio must be between 0 and 1".to_string());
        }
        if self.ttl.is_some_and(|duration| duration.is_zero())
            || self.tti.is_some_and(|duration| duration.is_zero())
        {
            return Err("cache expiration durations must be greater than 0".to_string());
        }
        Ok(())
    }
}
