use std::collections::HashMap;

use crate::planning::plan::PlanNodeEnum;

/// Optimization batch phase
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptimizationBatch {
    /// Normalize: canonical form transformations
    Normalize,
    /// Predicate pushdown: push filters down
    PredicatePushdown,
    /// Property pruning: remove unused properties
    PropertyPruning,
    /// Expand pushdown: annotate ExpandAll hops with id_only/count_only
    ExpandPushdown,
    /// Decorrelation: transform correlated subqueries
    Decorrelation,
    /// Cleanup: eliminate redundant operations
    Cleanup,
}

impl OptimizationBatch {
    /// Get the name of the batch
    pub fn name(&self) -> &'static str {
        match self {
            OptimizationBatch::Normalize => "normalize",
            OptimizationBatch::PredicatePushdown => "predicate_pushdown",
            OptimizationBatch::PropertyPruning => "property_pruning",
            OptimizationBatch::ExpandPushdown => "expand_pushdown",
            OptimizationBatch::Decorrelation => "decorrelation",
            OptimizationBatch::Cleanup => "cleanup",
        }
    }

    /// Get the maximum number of iterations for this batch
    pub fn default_max_iterations(&self) -> usize {
        match self {
            OptimizationBatch::Normalize => 10,
            OptimizationBatch::PredicatePushdown => 50,
            OptimizationBatch::PropertyPruning => 20,
            OptimizationBatch::ExpandPushdown => 10,
            OptimizationBatch::Decorrelation => 30,
            OptimizationBatch::Cleanup => 30,
        }
    }
}

/// Reason why a batch stopped
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BatchStopReason {
    /// Converged: no more changes
    #[default]
    Converged,
    /// Hit iteration limit
    IterationLimit(usize),
    /// Detected cycle/oscillation
    CycleDetected,
    /// Error during rewrite
    Error(String),
}

/// Statistics for a single batch execution
#[derive(Debug, Clone, Default)]
pub struct BatchStatistics {
    /// Number of iterations performed
    pub iterations: usize,
    /// Number of rules applied
    pub rules_applied: usize,
    /// Rule hit counts (rule_name -> count)
    pub rule_hit_counts: HashMap<String, usize>,
    /// Fingerprints before each iteration
    pub fingerprints_before: Vec<u64>,
    /// Fingerprints after each iteration
    pub fingerprints_after: Vec<u64>,
    /// Whether the batch converged
    pub converged: bool,
    /// Stop reason
    pub stop_reason: BatchStopReason,
}

/// Result of optimization with diagnostics
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    /// The optimized plan
    pub optimized_plan: PlanNodeEnum,
    /// Statistics for each batch
    pub batch_statistics: Vec<(OptimizationBatch, BatchStatistics)>,
    /// Total iterations across all batches
    pub total_iterations: usize,
    /// Total rules applied across all batches
    pub total_rules_applied: usize,
}

impl OptimizationResult {
    /// Get statistics for a specific batch
    pub fn batch_stats(&self, batch: OptimizationBatch) -> Option<&BatchStatistics> {
        self.batch_statistics
            .iter()
            .find(|(b, _)| *b == batch)
            .map(|(_, s)| s)
    }

    /// Check if any batch detected oscillation
    pub fn has_oscillation(&self) -> bool {
        self.batch_statistics
            .iter()
            .any(|(_, s)| matches!(s.stop_reason, BatchStopReason::CycleDetected))
    }

    /// Get a summary of the optimization
    pub fn summary(&self) -> String {
        format!(
            "Optimization completed: {} total iterations, {} rules applied, oscillation={}",
            self.total_iterations,
            self.total_rules_applied,
            self.has_oscillation()
        )
    }
}
