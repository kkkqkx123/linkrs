//! Freeze Statistics Collector
//!
//! Collects metrics about delta freezing operations and provides
//! configuration for freeze decision-making.
//!
//! Does NOT execute freezing — that's handled by GraphStorageContext
//! via trigger_background_freeze() method.
//!
//! ## Design
//!
//! BackgroundFreezeManager provides:
//! - Decision support via FreezeDecisionEngine (strategy-based should_freeze)
//! - Statistics collection (record_freeze, record_delta_size)
//!
//! Actual freezing is triggered by:
//! - GraphStorageContext::trigger_background_freeze() during maintenance
//! - Checkpoint operations before persistence
//! - HTTP API endpoint for manual triggering
//!
//! ## Example
//!
//! ```ignore
//! let config = FreezeConfig::production_small();
//! let manager = BackgroundFreezeManager::from_config(config);
//!
//! let input = FreezeDecisionInput {
//!     delta_edge_count: table.delta_edge_count(),
//!     delta_memory_bytes: table.used_memory_size() as u64,
//!     segment_count: table.segment_count(),
//!     oldest_segment_age: table.oldest_segment_age(),
//!     deletion_ratio: table.deletion_ratio(),
//! };
//!
//! if manager.should_freeze_with_stats(&input) {
//!     // Call trigger_background_freeze() externally
//! }
//! ```

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use super::config::{FreezeConfig, FreezeDecisionEngine, FreezeDecisionInput};

/// Automatic freeze statistics recording guard
///
/// When dropped, automatically records freeze completion in the manager.
/// Prevents missed statistics and ensures consistent tracking.
///
/// ## Example
///
/// ```ignore
/// let guard = FreezeGuard::new(manager.clone());
/// let edges_frozen = perform_freeze().await?;
/// guard.record_edges(edges_frozen);
/// // On drop, automatically records the freeze
/// ```
pub struct FreezeGuard {
    manager: Arc<BackgroundFreezeManager>,
    start_time: Instant,
    edges_frozen: u64,
    completed: bool,
}

impl FreezeGuard {
    /// Create a new freeze guard that will record statistics when dropped
    pub fn new(manager: Arc<BackgroundFreezeManager>) -> Self {
        Self {
            manager,
            start_time: Instant::now(),
            edges_frozen: 0,
            completed: false,
        }
    }

    /// Record the number of edges that were frozen
    pub fn record_edges(&mut self, count: u64) {
        self.edges_frozen = count;
        self.completed = true;
    }

    /// Manually complete the freeze (called automatically on drop)
    pub fn finish(&mut self) {
        if self.completed {
            let duration_ms = self.start_time.elapsed().as_millis() as u64;
            self.manager.record_freeze(self.edges_frozen, duration_ms);
            log::info!(
                "Freeze completed: {} edges in {}ms",
                self.edges_frozen,
                duration_ms
            );
        }
    }
}

impl Drop for FreezeGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Statistics about freeze operations
#[derive(Debug, Clone, Copy, Default)]
pub struct FreezeStats {
    /// Total number of freeze operations completed
    pub freeze_count: u64,
    /// Total edges frozen across all operations
    pub total_frozen_edges: u64,
    /// Duration of last freeze operation in milliseconds
    pub last_freeze_duration_ms: u64,
    /// Current mutable delta edges (unfrozen)
    pub current_delta_edges: u64,
}

/// Freeze decision information
#[derive(Debug, Clone)]
pub struct FreezeDecision {
    pub should_freeze: bool,
    pub current_delta_edges: u64,
    pub edge_threshold: u64,
    pub current_delta_memory_bytes: u64,
    pub memory_threshold_bytes: u64,
    pub freeze_reason: FreezeReason,
}

/// Reason why freeze was triggered
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FreezeReason {
    None,
    EdgeCountExceeded,
    MemoryExceeded,
    Both,
}

/// Freeze statistics collector and decision maker
///
/// Uses FreezeDecisionEngine for strategy-based decision making.
/// Provides decision support and metrics collection for delta freezing operations.
pub struct BackgroundFreezeManager {
    /// Decision engine using enum dispatch (no trait overhead)
    decision_engine: FreezeDecisionEngine,
    /// Statistics (thread-safe for concurrent reads)
    stats: Arc<Mutex<FreezeStats>>,
}


impl BackgroundFreezeManager {
    /// Create a new freeze manager from FreezeConfig (uses configured strategy)
    pub fn from_config(config: FreezeConfig) -> Self {
        let decision_engine = FreezeDecisionEngine::new(config.strategy, config);
        Self {
            decision_engine,
            stats: Arc::new(Mutex::new(FreezeStats::default())),
        }
    }

    /// Check if freezing should be triggered with full statistics
    pub fn should_freeze_with_stats(&self, input: &FreezeDecisionInput) -> bool {
        self.decision_engine.should_freeze(input)
    }

    /// Get detailed freeze decision
    pub fn get_freeze_decision_with_stats(&self, input: &FreezeDecisionInput) -> FreezeDecision {
        let edge_exceeded = input.delta_edge_count >= self.decision_engine.config.delta_edge_threshold;
        let memory_exceeded = input.delta_memory_bytes >= self.decision_engine.config.delta_memory_threshold_bytes;

        let freeze_reason = match (edge_exceeded, memory_exceeded) {
            (true, true) => FreezeReason::Both,
            (true, false) => FreezeReason::EdgeCountExceeded,
            (false, true) => FreezeReason::MemoryExceeded,
            (false, false) => FreezeReason::None,
        };

        FreezeDecision {
            should_freeze: self.should_freeze_with_stats(input),
            current_delta_edges: input.delta_edge_count,
            edge_threshold: self.decision_engine.config.delta_edge_threshold,
            current_delta_memory_bytes: input.delta_memory_bytes,
            memory_threshold_bytes: self.decision_engine.config.delta_memory_threshold_bytes,
            freeze_reason,
        }
    }

    /// Get current freeze statistics (snapshot)
    pub fn get_stats(&self) -> FreezeStats {
        *self.stats.lock()
    }

    /// Record a freeze event (called by GraphStorageContext after freezing)
    pub(crate) fn record_freeze(&self, edges_frozen: u64, duration_ms: u64) {
        let mut stats = self.stats.lock();
        stats.freeze_count += 1;
        stats.total_frozen_edges += edges_frozen;
        stats.last_freeze_duration_ms = duration_ms;
    }

    /// Record current delta size (for monitoring)
    pub(crate) fn record_delta_size(&self, delta_edges: u64) {
        let mut stats = self.stats.lock();
        stats.current_delta_edges = delta_edges;
    }

    /// Get strategy name
    pub fn strategy_name(&self) -> &'static str {
        self.decision_engine.strategy_name()
    }

    /// Get freeze reason as string
    pub fn get_reason(&self, input: &FreezeDecisionInput) -> String {
        self.decision_engine.get_reason(input)
    }

    /// Get configuration for testing/debugging
    pub fn get_config(&self) -> &FreezeConfig {
        &self.decision_engine.config
    }
}

impl Default for BackgroundFreezeManager {
    fn default() -> Self {
        Self::from_config(FreezeConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::FreezeStrategyType;

    #[test]
    fn test_freeze_decision_below_both_thresholds() {
        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        let decision = manager.get_freeze_decision_with_stats(&input);
        assert!(!decision.should_freeze);
        assert_eq!(decision.freeze_reason, FreezeReason::None);
    }

    #[test]
    fn test_freeze_decision_edge_count_exceeded() {
        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        let input = FreezeDecisionInput {
            delta_edge_count: 150_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        let decision = manager.get_freeze_decision_with_stats(&input);
        assert!(decision.should_freeze);
        assert_eq!(decision.freeze_reason, FreezeReason::EdgeCountExceeded);
    }

    #[test]
    fn test_freeze_decision_memory_exceeded() {
        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 300 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        let decision = manager.get_freeze_decision_with_stats(&input);
        assert!(decision.should_freeze);
        assert_eq!(decision.freeze_reason, FreezeReason::MemoryExceeded);
    }

    #[test]
    fn test_freeze_decision_both_exceeded() {
        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 100_000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        let input = FreezeDecisionInput {
            delta_edge_count: 150_000,
            delta_memory_bytes: 300 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        let decision = manager.get_freeze_decision_with_stats(&input);
        assert!(decision.should_freeze);
        assert_eq!(decision.freeze_reason, FreezeReason::Both);
    }

    #[test]
    fn test_should_freeze_with_stats_method() {
        let manager = BackgroundFreezeManager::default();

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };
        assert!(!manager.should_freeze_with_stats(&input));

        let input2 = FreezeDecisionInput {
            delta_edge_count: 150_000,
            ..input
        };
        assert!(manager.should_freeze_with_stats(&input2));
    }

    #[test]
    fn test_record_freeze() {
        let manager = BackgroundFreezeManager::default();

        manager.record_freeze(50_000, 100);
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 1);
        assert_eq!(stats.total_frozen_edges, 50_000);
        assert_eq!(stats.last_freeze_duration_ms, 100);

        manager.record_freeze(30_000, 50);
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 2);
        assert_eq!(stats.total_frozen_edges, 80_000);
        assert_eq!(stats.last_freeze_duration_ms, 50);
    }

    #[test]
    fn test_record_delta_size() {
        let manager = BackgroundFreezeManager::default();

        manager.record_delta_size(12_000);
        let stats = manager.get_stats();
        assert_eq!(stats.current_delta_edges, 12_000);

        manager.record_delta_size(25_000);
        let stats = manager.get_stats();
        assert_eq!(stats.current_delta_edges, 25_000);
    }

    #[test]
    fn test_manager_creation_and_stats() {
        let config = FreezeConfig {
            strategy: FreezeStrategyType::Conservative,
            delta_edge_threshold: 50_000,
            delta_memory_threshold_bytes: 128 * 1024 * 1024,
            max_segment_age: u32::MAX,
            deletion_threshold: 0.5,
            max_segment_size_bytes: 8 * 1024 * 1024,
            adaptive_segment_threshold: 50,
            adaptive_maximum_segments: 150,
            lsm_segment_pressure_threshold: 200,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 0);
        assert_eq!(stats.total_frozen_edges, 0);
        assert_eq!(stats.current_delta_edges, 0);
    }

    #[test]
    fn test_config_access() {
        let config = FreezeConfig::development();
        let manager = BackgroundFreezeManager::from_config(config.clone());

        let retrieved_config = manager.get_config();
        assert_eq!(retrieved_config.delta_edge_threshold, 50_000);
        assert_eq!(retrieved_config.delta_memory_threshold_bytes, 128 * 1024 * 1024);
    }

    #[test]
    fn test_default_config() {
        let manager = BackgroundFreezeManager::default();
        let config = manager.get_config();
        assert_eq!(
            config.delta_edge_threshold,
            100_000,
            "Default edge threshold should be 100K edges"
        );
        assert_eq!(
            config.delta_memory_threshold_bytes,
            256 * 1024 * 1024,
            "Default memory threshold should be 256MB"
        );
    }

    #[test]
    fn test_freeze_guard_records_automatically() {
        let manager = Arc::new(BackgroundFreezeManager::default());
        let stats_before = manager.get_stats();
        assert_eq!(stats_before.freeze_count, 0);

        // Create and use guard
        {
            let mut guard = FreezeGuard::new(manager.clone());
            guard.record_edges(50_000);
            // Guard automatically records on drop
        }

        // Verify statistics were recorded
        let stats_after = manager.get_stats();
        assert_eq!(stats_after.freeze_count, 1);
        assert_eq!(stats_after.total_frozen_edges, 50_000);
        assert!(stats_after.last_freeze_duration_ms < 1000);  // Should be fast
    }

    #[test]
    fn test_freeze_guard_without_edges_does_nothing() {
        let manager = Arc::new(BackgroundFreezeManager::default());

        // Create guard but don't call record_edges
        {
            let _guard = FreezeGuard::new(manager.clone());
            // Just drop without recording
        }

        // Should not record anything if completed=false
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 0);
    }

    #[test]
    fn test_freeze_guard_multiple() {
        let manager = Arc::new(BackgroundFreezeManager::default());

        // First freeze
        {
            let mut guard = FreezeGuard::new(manager.clone());
            guard.record_edges(30_000);
        }

        // Second freeze
        {
            let mut guard = FreezeGuard::new(manager.clone());
            guard.record_edges(40_000);
        }

        // Verify both recorded
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 2);
        assert_eq!(stats.total_frozen_edges, 70_000);
    }

    #[test]
    fn test_freeze_guard_manual_finish() {
        let manager = Arc::new(BackgroundFreezeManager::default());

        let mut guard = FreezeGuard::new(manager.clone());
        guard.record_edges(50_000);
        guard.finish();  // Manual finish
        // Guard is still dropped after this

        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 1);
        assert_eq!(stats.total_frozen_edges, 50_000);
    }
}
