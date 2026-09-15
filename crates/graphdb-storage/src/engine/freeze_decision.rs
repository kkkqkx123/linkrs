use crate::engine::config::FreezeConfig;

/// Input for freeze decision-making (minimal required statistics).
///
/// Single-segment CSR keeps no segments: the mutable CSR size (edge count
/// plus memory) and the deletion ratio drive the decision.
#[derive(Debug, Clone)]
pub struct FreezeDecisionInput {
    pub delta_edge_count: u64,
    pub delta_memory_bytes: u64,
    pub deletion_ratio: f64,
}

/// Decision engine for single-segment CSR compaction ("freeze").
///
/// Uses plain threshold checks instead of trait dispatch to keep the
/// decision logic centralized and clear.
pub struct FreezeDecisionEngine {
    pub(crate) config: FreezeConfig,
}

impl FreezeDecisionEngine {
    /// Create a new decision engine for the given config
    pub fn new(config: FreezeConfig) -> Self {
        Self { config }
    }

    /// Determine if freeze should be triggered: any threshold breach
    /// (edge count, memory, or deletion ratio) triggers compaction.
    pub fn should_freeze(&self, input: &FreezeDecisionInput) -> bool {
        input.delta_edge_count >= self.config.delta_edge_threshold
            || input.delta_memory_bytes >= self.config.delta_memory_threshold_bytes
            || input.deletion_ratio >= self.config.deletion_threshold
    }

    /// Get human-readable reason for freeze decision (for logging)
    pub fn get_reason(&self, input: &FreezeDecisionInput) -> String {
        if !self.should_freeze(input) {
            return "No freeze needed".to_string();
        }

        format!(
            "Freeze: edges={}/{}, memory={:.0}MB/{:.0}MB, deletions={:.2}/{}",
            input.delta_edge_count,
            self.config.delta_edge_threshold,
            input.delta_memory_bytes as f64 / 1024.0 / 1024.0,
            self.config.delta_memory_threshold_bytes as f64 / 1024.0 / 1024.0,
            input.deletion_ratio,
            self.config.deletion_threshold,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::config::FreezeConfig;

    fn input(edges: u64, mem_mb: u64, deletion_ratio: f64) -> FreezeDecisionInput {
        FreezeDecisionInput {
            delta_edge_count: edges,
            delta_memory_bytes: mem_mb * 1024 * 1024,
            deletion_ratio,
        }
    }

    #[test]
    fn test_freeze_decision_engine_conservative_edges() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(config);

        let below = input(30_000, 100, 0.1);
        assert!(!engine.should_freeze(&below));

        let above = input(60_000, 100, 0.1);
        assert!(engine.should_freeze(&above));
        assert!(engine.get_reason(&above).contains("edges="));
    }

    #[test]
    fn test_freeze_decision_engine_conservative_memory() {
        let config = FreezeConfig::development();
        let engine = FreezeDecisionEngine::new(config);

        let below = input(30_000, 100, 0.1);
        assert!(!engine.should_freeze(&below));

        let above = input(30_000, 200, 0.1);
        assert!(engine.should_freeze(&above));
    }

    #[test]
    fn test_freeze_decision_engine_deletion_ratio_triggers() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(config);

        // Below every threshold: no freeze.
        let clean = input(50_000, 200, 0.1);
        assert!(!engine.should_freeze(&clean));

        // Deletion ratio alone triggers compaction so high-churn tables
        // reclaim physical space without waiting for size thresholds.
        let dirty = input(50_000, 200, 0.25);
        assert!(engine.should_freeze(&dirty));
        assert!(engine.get_reason(&dirty).contains("deletions="));
    }

    #[test]
    fn test_freeze_decision_engine_no_freeze_without_breach() {
        let config = FreezeConfig::production_small();
        let engine = FreezeDecisionEngine::new(config);

        let input = input(50_000, 200, 0.15);
        assert!(!engine.should_freeze(&input));
        assert_eq!(engine.get_reason(&input), "No freeze needed");
    }
}
