use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cache::config::CachePriority;

/// CTE cache entries
#[derive(Debug, Clone)]
pub struct CteCacheEntry {
    /// Resulting data (shared using Arc)
    pub data: Arc<Vec<u8>>,
    /// Number of result rows
    pub row_count: u64,
    /// Result size (bytes)
    pub data_size: usize,
    /// Creation time
    pub created_at: Instant,
    /// Last access time
    pub last_accessed: Instant,
    /// Number of visits
    pub access_count: u64,
    /// Estimated probability of reuse
    pub reuse_probability: f64,
    /// CTE definition hash (used to identify identical CTEs)
    pub cte_hash: String,
    /// CTE definition text
    pub cte_definition: String,
    /// Cache priority
    pub priority: CachePriority,
    /// Compute cost (milliseconds)
    pub compute_cost_ms: u64,
    /// Access frequency (per minute)
    pub access_frequency: f64,
    /// Dependent tables (for invalidation detection)
    pub dependent_tables: Vec<String>,
}

impl CteCacheEntry {
    /// Creating a new cache entry
    pub fn new(
        cte_hash: String,
        cte_definition: String,
        data: Vec<u8>,
        row_count: u64,
        compute_cost_ms: u64,
    ) -> Self {
        let data_size = data.len();
        Self {
            data: Arc::new(data),
            row_count,
            data_size,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
            reuse_probability: 0.5,
            cte_hash,
            cte_definition,
            priority: CachePriority::Normal,
            compute_cost_ms,
            access_frequency: 0.0,
            dependent_tables: Vec::new(),
        }
    }

    /// Estimate memory usage (bytes)
    pub fn estimate_memory(&self) -> usize {
        let mut total = 0;

        total += std::mem::size_of::<Arc<Vec<u8>>>();
        total += std::mem::size_of::<Vec<u8>>();
        total += self.data_size;

        total += std::mem::size_of::<String>();
        total += self.cte_hash.capacity();

        total += std::mem::size_of::<String>();
        total += self.cte_definition.capacity();

        total += std::mem::size_of::<Vec<String>>();
        for table in &self.dependent_tables {
            total += std::mem::size_of::<String>();
            total += table.capacity();
        }

        total += std::mem::size_of::<Instant>() * 2;
        total += std::mem::size_of::<u64>() * 3;
        total += std::mem::size_of::<f64>() * 2;
        total += std::mem::size_of::<CachePriority>();

        total
    }

    /// Calculate cache value score (for eviction decisions)
    pub fn value_score(&self) -> f64 {
        let frequency_score = self.access_frequency * 0.4;
        let cost_score = (self.compute_cost_ms as f64 / 1000.0) * 0.3;
        let priority_score = (self.priority as i32 as f64) * 0.2;
        let size_penalty = (self.data_size as f64 / 1024.0 / 1024.0) * 0.1;

        frequency_score + cost_score + priority_score - size_penalty
    }

    /// Recorded visits
    pub fn record_access(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;

        let elapsed_minutes = self.created_at.elapsed().as_secs_f64() / 60.0;
        if elapsed_minutes > 0.0 {
            self.access_frequency = self.access_count as f64 / elapsed_minutes;
        }

        self.reuse_probability = (self.reuse_probability * 0.7 + 0.3).min(0.95);
    }

    /// Get Cache Age
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Getting free time
    pub fn idle_time(&self) -> Duration {
        self.last_accessed.elapsed()
    }

    /// Calculate cache score (for LRU elimination decisions)
    /// The lower the score, the more likely you are to be eliminated
    pub fn cache_score(&self) -> f64 {
        let idle_factor = self.idle_time().as_secs_f64() / 60.0;
        let size_factor = (self.data_size as f64 / 1024.0 / 1024.0).max(0.1);
        let access_factor = (self.access_count as f64).sqrt().max(1.0);

        (idle_factor * size_factor) / (access_factor * self.reuse_probability)
    }
}
