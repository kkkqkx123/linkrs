use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::plan_rewriter::NodeRewriter;
use crate::optimizer::heuristic::result::RewriteResult;
use crate::optimizer::heuristic::rule_enum::{RewriteRule, RuleRegistry};
use crate::planning::plan::PlanNodeEnum;

use super::rewriter::BatchNodeRewriter;
use super::types::{BatchStatistics, BatchStopReason, OptimizationBatch, OptimizationResult};

/// Maximum number of chain passes over all batches.
///
/// The batch chain can legitimately need more than one pass to reach a
/// fixed point (later batches undo earlier ones).  This bounds the outer
/// pass loop so pathological plans terminate; a boundary fingerprint that
/// repeats an older boundary is reported as cross-batch oscillation.
const MAX_CHAIN_PASSES: usize = 3;

/// Batch optimizer - applies rules in phases with diagnostics
#[derive(Debug)]
pub struct BatchOptimizer {
    /// Rules for each batch phase
    batch_rules: HashMap<OptimizationBatch, Vec<RewriteRule>>,
    /// Global maximum iterations across all batches
    max_iterations: AtomicUsize,
    /// Enable diagnostics tracking
    enable_diagnostics: bool,
}

impl Default for BatchOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of comparing a chain-boundary fingerprint against the history of
/// previously seen boundary fingerprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryOutcome {
    /// The fingerprint is new; the chain should continue.
    Progress,
    /// The fingerprint repeats the immediately preceding boundary: the chain
    /// reached a fixed point and stops.
    Converged,
    /// The fingerprint repeats an older boundary: the chain oscillates
    /// across batches instead of converging.
    Oscillation,
}

/// Classify a chain-boundary fingerprint against the boundary history.
///
/// A repeat of the last entry means the chain output equals its input
/// (fixed point).  A repeat of an entry further back means the chain cycles
/// through at least two distinct states across passes (cross-batch
/// oscillation).
fn classify_boundary_repeat(history: &[u64], fingerprint: u64) -> BoundaryOutcome {
    match history.iter().position(|fp| *fp == fingerprint) {
        None => BoundaryOutcome::Progress,
        Some(index) if index + 1 == history.len() => BoundaryOutcome::Converged,
        Some(_) => BoundaryOutcome::Oscillation,
    }
}

impl BatchOptimizer {
    /// Create a new batch optimizer with default configuration
    pub fn new() -> Self {
        Self {
            batch_rules: HashMap::new(),
            max_iterations: AtomicUsize::new(100),
            enable_diagnostics: true,
        }
    }

    /// Create a new batch optimizer from a rule registry
    pub fn from_registry(registry: RuleRegistry) -> Self {
        let mut optimizer = Self::new();

        // Default batch assignment based on rule categories
        let mut total_rules = 0usize;
        for rule in registry.into_vec() {
            let batch = Self::assign_rule_to_batch(&rule);
            optimizer.batch_rules.entry(batch).or_default().push(rule);
            total_rules += 1;
        }

        let assigned: usize = optimizer.batch_rules.values().map(Vec::len).sum();
        if assigned != total_rules {
            log::warn!(
                "BatchOptimizer: {}/{} rules unclassified and will not run",
                total_rules.saturating_sub(assigned),
                total_rules
            );
        }

        optimizer
    }

    /// Assign a rule to the appropriate batch
    fn assign_rule_to_batch(rule: &RewriteRule) -> OptimizationBatch {
        use crate::optimizer::heuristic::rule_enum::RewriteRule::*;

        match rule {
            // Cleanup batch: elimination and merge rules
            EliminateFilter(_)
            | RemoveNoopProject(_)
            | EliminateAppendVertices(_)
            | RemoveAppendVerticesBelowJoin(_)
            | EliminateRowCollect(_)
            | EliminateEmptySetOperation(_)
            | DedupElimination(_)
            | EliminateSort(_)
            | CombineFilter(_)
            | CollapseProject(_)
            | CollapseConsecutiveProject(_)
            | MergeGetVerticesAndProject(_)
            | MergeGetVerticesAndDedup(_)
            | MergeGetNbrsAndProject(_)
            | MergeGetNbrsAndDedup(_) => OptimizationBatch::Cleanup,

            // Predicate pushdown batch
            PushFilterDownTraverse(_)
            | PushFilterDownExpandAll(_)
            | PushFilterDownNode(_)
            | PushEFilterDown(_)
            | PushVFilterDownScanVertices(_)
            | PushFilterDownScanVertices(_)
            | PushFilterDownScanEdges(_)
            | EliminateRedundantTagFilter(_)
            | PushFilterDownInnerJoin(_)
            | PushFilterDownLeftJoin(_)
            | PushFilterDownCrossJoin(_)
            | PushFilterDownGetNbrs(_)
            | PushFilterDownAllPaths(_)
            | PushFilterDownAggregate(_) => OptimizationBatch::PredicatePushdown,

            // Property pruning batch
            PushProjectDownScanVertices(_)
            | PushProjectDownScanEdges(_)
            | PushProjectDownGetVertices(_)
            | PushProjectDownGetNeighbors(_)
            | PushProjectDownGetEdges(_)
            | PushProjectDownAppendVertices(_)
            | EnrichScanSlotsWithFilterProps(_) => OptimizationBatch::PropertyPruning,

            // Expand pushdown batch: whole-plan annotation of traversal hops.
            ExpandPushdownAnnotate(_) | ScanIdentityAnnotate(_) | ScanEdgeIdentityAnnotate(_) => {
                OptimizationBatch::ExpandPushdown
            }

            // Limit pushdown (part of normalize)
            PushLimitDownGetVertices(_)
            | PushLimitDownGetEdges(_)
            | PushLimitDownScanVertices(_)
            | PushLimitDownScanEdges(_)
            | PushLimitDownIndexScan(_)
            | PushTopNDownIndexScan(_)
            | ConvertSortLimitToTopN(_)
            // Constant folding (part of normalize)
            | FoldConstants(_) => OptimizationBatch::Normalize,

            // Join optimization (part of normalize/cleanup)
            PushProjectDownJoin(_)
            | ExtractEquiJoinKeys(_)
            | LeftJoinToInnerJoin(_)
            | JoinConditionSimplify(_)
            | JoinToExpand(_)
            | JoinToAppendVertices(_)
            | MergeConsecutiveExpand(_)
            | JoinElimination(_)
            | IndexJoinSelection(_)
            | JoinReorder(_) => OptimizationBatch::Normalize,

            // Decorrelation batch: stat-free subquery unnesting gate.
            UnnestSimplePatternApply(_) => OptimizationBatch::Decorrelation,
        }
    }

    /// Set the maximum number of iterations
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = AtomicUsize::new(max);
        self
    }

    /// Set the maximum number of iterations (interior mutability via `AtomicUsize`).
    pub fn set_max_iterations(&self, max: usize) {
        self.max_iterations.store(max, Ordering::Relaxed);
    }

    /// Set whether to enable diagnostics
    pub fn with_diagnostics(mut self, enable: bool) -> Self {
        self.enable_diagnostics = enable;
        self
    }

    /// Add rules for a specific batch
    pub fn add_batch_rules(&mut self, batch: OptimizationBatch, rules: Vec<RewriteRule>) {
        self.batch_rules.insert(batch, rules);
    }

    /// Get rules for a specific batch
    pub fn batch_rules(&self, batch: OptimizationBatch) -> Option<&Vec<RewriteRule>> {
        self.batch_rules.get(&batch)
    }

    /// Optimize a plan through all enabled batches
    ///
    /// The batch chain itself is repeated until the whole-plan fingerprint
    /// at a chain boundary stabilizes.  A single pass is not guaranteed to
    /// reach a fixed point because later batches can undo the work of
    /// earlier ones (e.g. a Cleanup merge that re-opens a predicate that
    /// PredicatePushdown pushed again).  The passes are bounded by
    /// [`MAX_CHAIN_PASSES`]; a boundary fingerprint that repeats an older
    /// boundary (not the immediately preceding one) is reported as a
    /// cross-batch oscillation via `BatchStopReason::CycleDetected` on the
    /// last batch's statistics.
    pub fn optimize(&self, plan: PlanNodeEnum) -> RewriteResult<OptimizationResult> {
        let mut current_plan = plan;
        let mut batch_stats: Vec<(OptimizationBatch, BatchStatistics)> = Vec::new();

        // Define batch execution order
        let batch_order = [
            OptimizationBatch::Normalize,
            OptimizationBatch::PredicatePushdown,
            OptimizationBatch::PropertyPruning,
            OptimizationBatch::ExpandPushdown,
            OptimizationBatch::Decorrelation,
            OptimizationBatch::Cleanup,
        ];
        let enabled: Vec<OptimizationBatch> = batch_order
            .iter()
            .copied()
            .filter(|batch| {
                self.batch_rules
                    .get(batch)
                    .is_some_and(|rules| !rules.is_empty())
            })
            .collect();

        if enabled.is_empty() {
            return Ok(OptimizationResult {
                optimized_plan: current_plan,
                batch_statistics: batch_stats,
                total_iterations: 0,
                total_rules_applied: 0,
            });
        }

        // Whole-plan fingerprint at each chain boundary.  A repeat of the
        // immediately previous boundary means the chain reached a fixed
        // point; a repeat of an older boundary means the chain oscillates
        // across batches instead of converging.
        let mut boundary_fingerprints: Vec<u64> = Vec::new();

        for pass in 0..MAX_CHAIN_PASSES {
            let boundary_fp = Self::calculate_fingerprint(&current_plan);
            match classify_boundary_repeat(&boundary_fingerprints, boundary_fp) {
                BoundaryOutcome::Converged => {
                    log::debug!("Batch chain reached a fixed point after {pass} pass(es)");
                    break;
                }
                BoundaryOutcome::Oscillation => {
                    log::warn!(
                        "Cross-batch oscillation detected: fingerprint of pass {pass} repeats an older boundary",
                    );
                    if let Some((_, last_stats)) = batch_stats.last_mut() {
                        last_stats.stop_reason = BatchStopReason::CycleDetected;
                    }
                    break;
                }
                BoundaryOutcome::Progress => {
                    boundary_fingerprints.push(boundary_fp);
                }
            }

            log::debug!("Starting optimization chain pass {pass}");
            for batch in &enabled {
                let rules = self
                    .batch_rules
                    .get(batch)
                    .expect("enabled batch has rules");
                log::debug!("Starting optimization batch: {}", batch.name());

                let (optimized_plan, stats) = self.execute_batch(
                    current_plan,
                    rules,
                    *batch,
                    batch.default_max_iterations(),
                )?;

                log::debug!(
                    "Batch {} completed: {} iterations, {} rules applied, converged={}",
                    batch.name(),
                    stats.iterations,
                    stats.rules_applied,
                    stats.converged
                );

                current_plan = optimized_plan;
                batch_stats.push((*batch, stats));
            }
        }

        let total_iterations: usize = batch_stats.iter().map(|(_, s)| s.iterations).sum();
        let total_rules_applied: usize = batch_stats.iter().map(|(_, s)| s.rules_applied).sum();

        Ok(OptimizationResult {
            optimized_plan: current_plan,
            batch_statistics: batch_stats,
            total_iterations,
            total_rules_applied,
        })
    }

    /// Execute a single optimization batch
    fn execute_batch(
        &self,
        plan: PlanNodeEnum,
        rules: &[RewriteRule],
        batch: OptimizationBatch,
        max_iterations: usize,
    ) -> RewriteResult<(PlanNodeEnum, BatchStatistics)> {
        let mut current_plan = plan;
        let mut stats = BatchStatistics::default();
        let mut fingerprints_seen = std::collections::HashSet::new();

        // `max_iterations` is the batch's tuned budget from
        // `OptimizationBatch::default_max_iterations()`. Treat it as
        // authoritative for both the outer fixed-point loop and the inner
        // per-node rewrite loop, so a single cap governs the whole batch.
        // The previous code shadowed this with `min(global_atomic)`, which let
        // a mutated global ceiling shrink every batch below its own tuned
        // default (min-cap dominance) and left the per-node loop using the
        // raw, un-capped global — inconsistent with the batch-level limit.
        for iteration in 0..max_iterations {
            // Calculate fingerprint before iteration
            let fingerprint_before = Self::calculate_fingerprint(&current_plan);
            if self.enable_diagnostics {
                stats.fingerprints_before.push(fingerprint_before);
            }

            // Check for cycles/oscillation
            if !fingerprints_seen.insert(fingerprint_before) {
                stats.stop_reason = BatchStopReason::CycleDetected;
                stats.converged = false;
                log::warn!(
                    "Batch {} detected cycle/oscillation after {} iterations",
                    batch.name(),
                    iteration
                );
                break;
            }

            // Execute one iteration
            let (new_plan, rules_applied, rule_hits) =
                self.batch_iteration(current_plan, rules, batch, max_iterations)?;

            // Calculate fingerprint after iteration
            let fingerprint_after = Self::calculate_fingerprint(&new_plan);
            if self.enable_diagnostics {
                stats.fingerprints_after.push(fingerprint_after);
            }

            // Update statistics
            stats.iterations = iteration + 1;
            stats.rules_applied += rules_applied;
            for (rule_name, count) in rule_hits {
                *stats.rule_hit_counts.entry(rule_name).or_insert(0) += count;
            }

            // Check for convergence
            if fingerprint_before == fingerprint_after {
                stats.stop_reason = BatchStopReason::Converged;
                stats.converged = true;
                log::debug!(
                    "Batch {} converged after {} iterations",
                    batch.name(),
                    iteration + 1
                );
                current_plan = new_plan;
                break;
            }

            current_plan = new_plan;
        }

        Ok((current_plan, stats))
    }

    /// Execute one iteration of a batch.
    ///
    /// Walks the whole plan tree bottom-up, applying the batch's rules at
    /// every node (not only at the root), so pushdown and merge rules are
    /// effective for aggregate/grouping-rooted queries.
    fn batch_iteration(
        &self,
        plan: PlanNodeEnum,
        rules: &[RewriteRule],
        _batch: OptimizationBatch,
        max_iterations: usize,
    ) -> RewriteResult<(
        PlanNodeEnum,
        usize,
        std::collections::HashMap<String, usize>,
    )> {
        let mut ctx = RewriteContext::new();
        let root_id = ctx.allocate_node_id();
        let rewriter = BatchNodeRewriter::new(rules, max_iterations);
        let new_plan = rewriter.rewrite_node(&mut ctx, &plan, root_id)?;
        let (rules_applied, rule_hits) = rewriter.into_counts();
        Ok((new_plan, rules_applied, rule_hits))
    }

    /// Calculate fingerprint for a plan node
    fn calculate_fingerprint(node: &PlanNodeEnum) -> u64 {
        use crate::optimizer::analysis::FingerprintCalculator;
        let calculator = FingerprintCalculator::new();
        calculator.calculate_fingerprint(node).value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::heuristic::rule_enum::RuleRegistry;

    #[test]
    fn test_batch_optimizer_creation() {
        let optimizer = BatchOptimizer::new();
        assert_eq!(optimizer.max_iterations.load(Ordering::Relaxed), 100);
        assert!(optimizer.enable_diagnostics);
    }

    #[test]
    fn test_batch_optimizer_from_registry() {
        let registry = RuleRegistry::default();
        let total_rules = registry.len();
        let optimizer = BatchOptimizer::from_registry(registry);

        // Check that rules are distributed across batches
        let assigned_rules: usize = optimizer
            .batch_rules
            .values()
            .map(|rules| rules.len())
            .sum();

        assert!(assigned_rules > 0);
        // Every rule in the registry must land in exactly one batch.
        assert_eq!(assigned_rules, total_rules);
    }

    #[test]
    fn test_batch_assignment() {
        use crate::optimizer::heuristic::elimination::EliminateFilterRule;
        use crate::optimizer::heuristic::predicate_pushdown::PushFilterDownNodeRule;

        let cleanup_rule = RewriteRule::EliminateFilter(EliminateFilterRule::new());
        let pushdown_rule = RewriteRule::PushFilterDownNode(PushFilterDownNodeRule::new());

        assert_eq!(
            BatchOptimizer::assign_rule_to_batch(&cleanup_rule),
            OptimizationBatch::Cleanup
        );
        assert_eq!(
            BatchOptimizer::assign_rule_to_batch(&pushdown_rule),
            OptimizationBatch::PredicatePushdown
        );
    }

    #[test]
    fn test_optimization_result_summary() {
        let result = OptimizationResult {
            optimized_plan: crate::planning::plan::core::nodes::PlanNodeEnum::Start(
                crate::planning::plan::core::nodes::StartNode::new(),
            ),
            batch_statistics: Vec::new(),
            total_iterations: 10,
            total_rules_applied: 25,
        };

        let summary = result.summary();
        assert!(summary.contains("10 total iterations"));
        assert!(summary.contains("25 rules applied"));
    }

    #[test]
    fn test_classify_boundary_repeat() {
        // A fresh fingerprint makes progress.
        assert_eq!(classify_boundary_repeat(&[], 1), BoundaryOutcome::Progress);
        assert_eq!(
            classify_boundary_repeat(&[1, 2], 3),
            BoundaryOutcome::Progress
        );
        // Repeating the immediately preceding boundary is a fixed point.
        assert_eq!(
            classify_boundary_repeat(&[1], 1),
            BoundaryOutcome::Converged
        );
        assert_eq!(
            classify_boundary_repeat(&[1, 2, 3], 3),
            BoundaryOutcome::Converged
        );
        // Repeating an older boundary is cross-batch oscillation.
        assert_eq!(
            classify_boundary_repeat(&[1, 2], 1),
            BoundaryOutcome::Oscillation
        );
        assert_eq!(
            classify_boundary_repeat(&[1, 2, 3, 4], 2),
            BoundaryOutcome::Oscillation
        );
    }

    #[test]
    fn test_optimize_with_real_rules_is_idempotent_and_terminates() {
        // The default rule registry, applied to a plan with a pushable
        // filter, must converge within the bounded chain passes: the
        // optimized plan is unchanged when optimized again (fixed point),
        // and no cross-batch oscillation is reported.
        use crate::optimizer::analysis::FingerprintCalculator;
        use crate::optimizer::heuristic::rule_enum::RuleRegistry;
        use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
        use linkrs_core::types::expr::ExpressionMeta;
        use linkrs_core::Expression;
        use linkrs_core::Value;
        use std::sync::Arc;

        let optimizer = BatchOptimizer::from_registry(RuleRegistry::default());

        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        let input = PlanNodeEnum::ScanVertices(scan);

        let expr_ctx = Arc::new(linkrs_core::types::expr::ExpressionAnalysisContext::new());
        let condition = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "age".to_string(),
            }),
            op: linkrs_core::types::BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(18))),
        };
        let id = expr_ctx.register_expression(ExpressionMeta::new(condition));
        let condition = linkrs_core::types::ContextualExpression::new(id, expr_ctx);
        let plan = PlanNodeEnum::Filter(
            FilterNode::new(input, condition).expect("FilterNode creation should succeed"),
        );

        let result = optimizer
            .optimize(plan)
            .expect("optimization should succeed");
        assert!(!result.has_oscillation());
        assert!(result.total_iterations > 0);

        let again = optimizer
            .optimize(result.optimized_plan.clone())
            .expect("second optimization should succeed");
        let calculator = FingerprintCalculator::new();
        assert_eq!(
            calculator.calculate_fingerprint(&again.optimized_plan),
            calculator.calculate_fingerprint(&result.optimized_plan),
            "optimization must reach a fixed point"
        );
    }
}
