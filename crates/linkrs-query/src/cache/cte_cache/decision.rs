use std::sync::Arc;

use crate::cache::config::CachePriority;

use super::manager::CteCacheManager;

/// CTE Cache Decision
///
/// Decide whether to use caching based on query characteristics
#[derive(Debug, Clone)]
pub struct CteCacheDecision {
    /// Should the cache be used?
    pub should_cache: bool,
    /// Reasons for decision-making
    pub reason: String,
    /// Estimated reuse probability
    pub reuse_probability: f64,
    /// Estimated caching gains
    pub estimated_benefit: f64,
    /// Suggested priority
    pub suggested_priority: CachePriority,
}

/// CTE cache decision maker
#[derive(Debug)]
pub struct CteCacheDecisionMaker {
    /// Cache Manager
    cache_manager: Arc<CteCacheManager>,
    /// Minimum Reuse Probability Threshold
    min_reuse_probability: f64,
    /// Minimum estimated gain
    min_benefit: f64,
}

impl CteCacheDecisionMaker {
    /// Create a new decision maker.
    pub fn new(cache_manager: Arc<CteCacheManager>) -> Self {
        Self {
            cache_manager,
            min_reuse_probability: 0.3,
            min_benefit: 1.0,
        }
    }

    /// Setting parameters
    pub fn with_params(mut self, min_reuse_probability: f64, min_benefit: f64) -> Self {
        self.min_reuse_probability = min_reuse_probability;
        self.min_benefit = min_benefit;
        self
    }

    /// Making a decision regarding caching
    pub fn decide(
        &self,
        cte_definition: &str,
        estimated_rows: u64,
        compute_cost: f64,
    ) -> CteCacheDecision {
        if !self.cache_manager.is_enabled() {
            return CteCacheDecision {
                should_cache: false,
                reason: "Cache disabled".to_string(),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let config = self.cache_manager.config();

        if estimated_rows < config.min_row_count {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Row count {} below minimum {}",
                    estimated_rows, config.min_row_count
                ),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        if estimated_rows > config.max_row_count {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Row count {} above maximum {}",
                    estimated_rows, config.max_row_count
                ),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let reuse_prob = self.cache_manager.predict_reuse_probability(cte_definition);

        if reuse_prob < self.min_reuse_probability {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Reuse probability {:.2} below threshold {:.2}",
                    reuse_prob, self.min_reuse_probability
                ),
                reuse_probability: reuse_prob,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let estimated_benefit = compute_cost * reuse_prob;

        let priority = if compute_cost > 1000.0 {
            CachePriority::High
        } else if compute_cost > 100.0 {
            CachePriority::Normal
        } else {
            CachePriority::Low
        };

        CteCacheDecision {
            should_cache: estimated_benefit >= self.min_benefit,
            reason: if estimated_benefit >= self.min_benefit {
                format!("Estimated benefit {:.2} meets threshold", estimated_benefit)
            } else {
                format!(
                    "Estimated benefit {:.2} below threshold {:.2}",
                    estimated_benefit, self.min_benefit
                )
            },
            reuse_probability: reuse_prob,
            estimated_benefit,
            suggested_priority: priority,
        }
    }
}
