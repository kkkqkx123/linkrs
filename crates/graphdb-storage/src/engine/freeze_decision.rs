use graphdb_core::types::Timestamp;

use crate::engine::config::{FreezeConfig, FreezeStrategyType};

/// Input for Freeze decision-making (minimal required statistics)
#[derive(Debug, Clone)]
pub struct FreezeDecisionInput {
    pub delta_edge_count: u64,
    pub delta_memory_bytes: u64,
    pub segment_count: usize,
    pub oldest_segment_age: Timestamp,
    pub deletion_ratio: f64,
}

/// Decision engine for Freeze strategy
///
/// Uses enum dispatch (match) instead of trait dispatch to:
/// - Keep decision logic centralized and clear
/// - Avoid trait overhead for simple configurations
/// - Reduce code complexity by 20-40%
pub struct FreezeDecisionEngine {
    pub(crate) strategy: FreezeStrategyType,
    pub(crate) config: FreezeConfig,
}

impl FreezeDecisionEngine {
    /// Create a new decision engine for the given strategy and config
    pub fn new(strategy: FreezeStrategyType, config: FreezeConfig) -> Self {
        Self { strategy, config }
    }

    /// Determine if freeze should be triggered based on strategy
    pub fn should_freeze(&self, input: &FreezeDecisionInput) -> bool {
        match self.strategy {
            FreezeStrategyType::Conservative => self.decide_conservative(input),
            FreezeStrategyType::Adaptive => self.decide_adaptive(input),
            FreezeStrategyType::LSMTiered => self.decide_lsm_tiered(input),
        }
    }

    /// Get human-readable reason for freeze decision (for logging)
    pub fn get_reason(&self, input: &FreezeDecisionInput) -> String {
        if !self.should_freeze(input) {
            return "No freeze needed".to_string();
        }

        match self.strategy {
            FreezeStrategyType::Conservative => {
                format!(
                    "Conservative: edges={}/{}, memory={:.0}MB/{:.0}MB",
                    input.delta_edge_count,
                    self.config.delta_edge_threshold,
                    input.delta_memory_bytes as f64 / 1024.0 / 1024.0,
                    self.config.delta_memory_threshold_bytes as f64 / 1024.0 / 1024.0
                )
            }
            FreezeStrategyType::Adaptive => {
                format!(
                    "Adaptive: edges={}/{}, age={}/{}, segments={}",
                    input.delta_edge_count,
                    self.config.delta_edge_threshold,
                    input.oldest_segment_age,
                    self.config.max_segment_age,
                    input.segment_count
                )
            }
            FreezeStrategyType::LSMTiered => {
                format!("LSMTiered: segments={}", input.segment_count)
            }
        }
    }

    /// Get strategy name
    pub fn strategy_name(&self) -> &'static str {
        match self.strategy {
            FreezeStrategyType::Conservative => "Conservative",
            FreezeStrategyType::Adaptive => "Adaptive",
            FreezeStrategyType::LSMTiered => "LSMTiered",
        }
    }

    // Private decision methods

    fn decide_conservative(&self, input: &FreezeDecisionInput) -> bool {
        input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes
    }

    fn decide_adaptive(&self, input: &FreezeDecisionInput) -> bool {
        // Condition 1: Base freeze (absolute thresholds)
        let base_freeze = input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes;

        // Condition 2: Too many segments (independent of age/deletion)
        let too_many_segments = input.segment_count >= self.config.adaptive_maximum_segments;

        // Condition 3: Old segments with high deletion ratio
        let old_and_dirty = input.oldest_segment_age > self.config.max_segment_age
            && input.deletion_ratio > self.config.deletion_threshold
            && input.segment_count >= self.config.adaptive_segment_threshold;

        base_freeze || too_many_segments || old_and_dirty
    }

    fn decide_lsm_tiered(&self, input: &FreezeDecisionInput) -> bool {
        // Base freeze: absolute thresholds (edge count or memory)
        let base_freeze = input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes;

        // LSM pressure: too many segments at any level
        let lsm_pressure = input.segment_count >= self.config.lsm_segment_pressure_threshold;

        base_freeze || lsm_pressure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::config::{FreezeConfig, FreezeStrategyType};

    #[test]
    fn test_freeze_decision_engine_conservative_edges() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 60_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
        assert!(engine.get_reason(&input).contains("Conservative"));
    }

    #[test]
    fn test_freeze_decision_engine_conservative_memory() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 30_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_conservative_no_freeze() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 30_000,
            delta_memory_bytes: 100 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 1000,
            deletion_ratio: 0.1,
        };

        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_base_threshold() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 150_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 50,
            oldest_segment_age: 2000,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_age_condition() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 75,        // Between threshold (50) and maximum (150)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.25,     // > 0.2
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_too_many_segments() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        // Test: Too many segments forces freeze (independent of age/deletion)
        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 150,      // At maximum_segments threshold
            oldest_segment_age: 100, // Below threshold
            deletion_ratio: 0.05,    // Below threshold
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_no_freeze_too_few_segments() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 30,        // Below adaptive_segment_threshold (50)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.25,     // > 0.2
        };

        // Should NOT freeze because segment count is below threshold
        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_adaptive_no_freeze_without_deletion() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 50_000,
            delta_memory_bytes: 200 * 1024 * 1024,
            segment_count: 75,        // Below maximum_segments (150)
            oldest_segment_age: 6000, // > 5000
            deletion_ratio: 0.15,     // < 0.2
        };

        assert!(!engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_lsm_tiered_segments() {
        let config = FreezeConfig::production_large();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 200_000,
            delta_memory_bytes: 500 * 1024 * 1024,
            segment_count: 250, // > 200
            oldest_segment_age: 500,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
        assert!(engine.get_reason(&input).contains("LSMTiered"));
    }

    #[test]
    fn test_freeze_decision_engine_lsm_tiered_base_threshold() {
        let config = FreezeConfig::production_large();
        let engine = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);

        let input = FreezeDecisionInput {
            delta_edge_count: 600_000, // > 500_000
            delta_memory_bytes: 500 * 1024 * 1024,
            segment_count: 150, // < 200
            oldest_segment_age: 500,
            deletion_ratio: 0.1,
        };

        assert!(engine.should_freeze(&input));
    }

    #[test]
    fn test_freeze_decision_engine_strategy_names() {
        let config = FreezeConfig::development();

        let engine_conservative =
            FreezeDecisionEngine::new(FreezeStrategyType::Conservative, config.clone());
        assert_eq!(engine_conservative.strategy_name(), "Conservative");

        let engine_adaptive =
            FreezeDecisionEngine::new(FreezeStrategyType::Adaptive, config.clone());
        assert_eq!(engine_adaptive.strategy_name(), "Adaptive");

        let engine_lsm = FreezeDecisionEngine::new(FreezeStrategyType::LSMTiered, config);
        assert_eq!(engine_lsm.strategy_name(), "LSMTiered");
    }
}
