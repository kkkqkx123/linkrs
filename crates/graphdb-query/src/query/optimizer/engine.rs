//! Optimizer Engine Module
//!
//! This module provides a query optimization engine, which is responsible for coordinating and managing all components related to query optimization.
//!
//! ## Design Specifications
//!
//! `OptimizerEngine` is the core component of the query optimization layer and is shared and used wherever it is needed through dependency injection.
//! It integrates functions such as statistical information management, cost calculation, and selective estimation, providing a unified optimization service for the query pipeline.
//!
//! ## Explanation of Shared Instances
//!
//! The `OptimizerEngine` is designed to be a component that can be shared across multiple queries for the following reasons:
//!
//! 1. **Sharing of statistical information**: All queries share the same set of statistical information, ensuring consistency in cost estimates.
//! 2. **Resource Efficiency**: Avoid the repeated creation of optimizer components in each query pipeline.
//! 3. **Configuration Consistency**: A unified cost model configuration is applied to all queries.
//!
//! ## How to use it
//!
//! ```rust
// Created during the initialization of the database instance
//! let optimizer_engine = Arc::new(OptimizerEngine::new(CostModelConfig::default()));
//!
// Used in the query pipeline through dependency injection
//! let pipeline = QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine);
//! ```
//!
//! ## Thread Safety
//!
//! `OptimizerEngine` utilizes `Arc` as well as thread-safe data structures, which allow for safe sharing in a multi-threaded environment.
//!
//! ## Attention
//!
//! This is not a global singleton, but an instance that is shared between components through `Arc`. Each database instance can have its own optimizer engine configuration.

use std::sync::Arc;
use std::sync::Mutex;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::optimizer::cost_based::subquery_unnesting::UnnestDecision;
use crate::query::optimizer::cost_based::{
    AggregateContext, AggregateStrategySelector, IndexSelector, SortEliminationOptimizer,
};
use crate::query::optimizer::heuristic::batch::{
    BatchOptimizer, BatchStatistics, OptimizationBatch,
};
use crate::query::optimizer::heuristic::rule_enum::RuleRegistry;
use crate::query::optimizer::partitioning::{PartitioningConfig, PartitioningPlanner};
use crate::query::optimizer::stats::StatsView;
use crate::query::optimizer::{
    BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager, SelectivityEstimator,
    StatisticsManager, SubqueryUnnestingOptimizer,
};

use crate::query::planning::plan::ExecutionPlan;
use crate::query::planning::plan::PlanNodeEnum;

/// Optimizer engine
///
/// A globally unique instance of the optimizer engine, responsible for coordinating and managing all components related to query optimization.
/// It has the same lifecycle as the database instance and provides unified optimization services for all queries.
#[derive(Debug)]
pub struct OptimizerEngine {
    /// Expression context, used for sharing expression information across different stages
    expression_context: Arc<ExpressionAnalysisContext>,
    /// Statistics Information Manager
    stats_manager: Arc<StatisticsManager>,
    /// CTE Cache Manager
    cte_cache_manager: Arc<CteCacheManager>,
    /// Cost Calculator
    cost_calculator: Arc<CostCalculator>,
    /// Selective Estimator
    selectivity_estimator: Arc<SelectivityEstimator>,
    /// Batch plan analyzer (unified analysis)
    batch_plan_analyzer: BatchPlanAnalyzer,
    /// Subquery de-correlating optimizer
    subquery_unnesting_optimizer: SubqueryUnnestingOptimizer,
    /// Cost model configuration
    cost_config: CostModelConfig,
    /// Heuristic batch optimizer (production heuristic main chain)
    heuristic_batch: BatchOptimizer,
    /// Last batch optimization statistics (exposed for EXPLAIN diagnostics)
    last_batch_statistics: Mutex<Vec<(OptimizationBatch, BatchStatistics)>>,
    /// Conservative selector for physical streaming partitions.
    partitioning_planner: PartitioningPlanner,
    /// Enable heuristic optimization phase
    enable_heuristic: bool,
    /// Maximum iterations for heuristic rules
    max_heuristic_iterations: usize,
}

impl OptimizerEngine {
    /// Create a new optimizer engine.
    ///
    /// # Parameters
    /// `cost_config`: Configuration of the cost model
    pub fn new(cost_config: CostModelConfig) -> Self {
        Self::with_expression_context(Arc::new(ExpressionAnalysisContext::new()), cost_config)
    }

    /// Create an optimizer engine using the shared ExpressionContext.
    ///
    /// # Parameters
    /// `expression_context`: A shared context for expressions (shared across different stages).
    /// - `cost_config`: Cost model configuration
    pub fn with_expression_context(
        expression_context: Arc<ExpressionAnalysisContext>,
        cost_config: CostModelConfig,
    ) -> Self {
        // Create a statistical information manager
        let stats_manager = Arc::new(StatisticsManager::new());

        // Create a CTE (Common Table Expression) for cache manager management.
        let cte_cache_manager = Arc::new(CteCacheManager::new());

        Self::with_components(
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_config,
        )
    }

    /// Create an optimizer engine with all components (used by builder).
    ///
    /// This internal constructor allows to builder pattern to inject custom components
    /// while maintaining backward compatibility with existing constructors.
    pub(crate) fn with_components(
        expression_context: Arc<ExpressionAnalysisContext>,
        stats_manager: Arc<StatisticsManager>,
        cte_cache_manager: Arc<CteCacheManager>,
        cost_config: CostModelConfig,
    ) -> Self {
        // Create a cost calculator and a selective estimator.
        let cost_calculator = Arc::new(CostCalculator::with_config(
            stats_manager.clone(),
            cost_config,
        ));
        let selectivity_estimator = Arc::new(SelectivityEstimator::new(stats_manager.clone()));

        // Create batch plan analyzer (unified analysis)
        let batch_plan_analyzer = BatchPlanAnalyzer::new();

        // Create a subquery to de-associate the optimizer.
        let subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();

        // Create the heuristic batch optimizer (production heuristic main chain)
        let heuristic_batch = BatchOptimizer::from_registry(RuleRegistry::default());

        Self {
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_calculator,
            selectivity_estimator,
            batch_plan_analyzer,
            subquery_unnesting_optimizer,
            cost_config,
            heuristic_batch,
            last_batch_statistics: Mutex::new(Vec::new()),
            partitioning_planner: PartitioningPlanner::new(PartitioningConfig::default()),
            enable_heuristic: true,
            max_heuristic_iterations: 100,
        }
    }

    /// Create an optimized configuration using an SSD.
    pub fn for_ssd() -> Self {
        Self::new(CostModelConfig::for_ssd())
    }

    /// Create an optimized configuration using a memory-based database.
    pub fn for_in_memory() -> Self {
        Self::new(CostModelConfig::for_in_memory())
    }

    /// Obtaining the Cost Model Configuration
    pub fn cost_config(&self) -> &CostModelConfig {
        &self.cost_config
    }

    /// Obtain the Cost Calculator
    pub fn cost_calculator(&self) -> &Arc<CostCalculator> {
        &self.cost_calculator
    }

    /// Statistics Information Manager
    pub fn stats_manager(&self) -> &Arc<StatisticsManager> {
        &self.stats_manager
    }

    /// Obtaining a selective estimator
    pub fn selectivity_estimator(&self) -> &Arc<SelectivityEstimator> {
        &self.selectivity_estimator
    }

    /// Obtain the context of the expression.
    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    /// Obtain batch plan analyzer
    pub fn batch_plan_analyzer(&self) -> &BatchPlanAnalyzer {
        &self.batch_plan_analyzer
    }

    /// Obtaining the subquery to de-associate the optimizer
    pub fn subquery_unnesting_optimizer(&self) -> &SubqueryUnnestingOptimizer {
        &self.subquery_unnesting_optimizer
    }

    /// Obtaining the CTE Cache Manager
    pub fn cte_cache_manager(&self) -> &CteCacheManager {
        &self.cte_cache_manager
    }

    /// Set the stats manager on the CTE cache manager
    pub fn set_cte_cache_stats_manager(
        &self,
        stats_manager: Arc<crate::core::stats::StatsManager>,
    ) {
        self.cte_cache_manager.set_stats_manager(stats_manager);
    }

    /// Update the Cost Model Configuration
    ///
    /// Updating the configuration will recreate the cost calculator, but it will not affect the existing decision cache.
    pub fn set_cost_config(&mut self, config: CostModelConfig) {
        self.cost_config = config;
        self.cost_calculator = Arc::new(CostCalculator::with_config(
            self.stats_manager.clone(),
            self.cost_config,
        ));
        // Re-create batch plan analyzer
        self.batch_plan_analyzer = BatchPlanAnalyzer::new();
        // Re-create the subquery to de-associate the optimizer.
        self.subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();
        log::info!(
            "Optimizer cost model configuration has been updated: {:?}",
            self.cost_config
        );
    }

    /// Set whether to enable heuristic optimization
    pub fn set_enable_heuristic(&mut self, enable: bool) {
        self.enable_heuristic = enable;
        log::info!(
            "Heuristic optimization has {}",
            if enable {
                "(computing) enable (a feature)"
            } else {
                "prohibit the use of sth."
            }
        );
    }

    /// Set the maximum number of heuristic iterations
    pub fn set_max_heuristic_iterations(&mut self, max: usize) {
        self.max_heuristic_iterations = max;
        log::info!(
            "The maximum number of heuristic iterations has been set to {}",
            max
        );
    }

    /// Optimize an execution plan through all enabled phases
    ///
    /// This is the main entry point for query optimization, coordinating both
    /// heuristic and cost-based optimization phases.
    ///
    /// # Parameters
    /// `plan`: The execution plan to optimize
    /// `space`: The space of the query being optimized; `None` disables
    /// statistics-driven cost-based decisions.
    ///
    /// # Returns
    /// The optimized execution plan
    pub fn optimize(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;

        // Phase 1: Heuristic Optimization (Always Executed)
        if self.enable_heuristic {
            log::debug!("Starting Phase 1: Heuristic Optimization");
            current_plan = self
                .apply_heuristic_with_max_iterations(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Phase 1 completed successfully");
        }

        // Phase 2: Cost-Based Optimization (always active — conservative rules)
        log::debug!("Starting Phase 2: Cost-Based Optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Phase 2 completed successfully");

        current_plan = self.apply_partitioning_selection(current_plan, space);

        Ok(current_plan)
    }

    fn apply_partitioning_selection(
        &self,
        mut plan: ExecutionPlan,
        space: Option<&str>,
    ) -> ExecutionPlan {
        if plan.partition_spec().is_some() {
            return plan;
        }
        let Some(root) = plan.root.as_ref() else {
            return plan;
        };
        let stats = StatsView::new(&self.stats_manager, space);
        let decision = self.partitioning_planner.decide(root, &stats);
        if let Some(spec) = decision.partition_spec {
            log::debug!("Selected partition layout: {}", decision.reason);
            plan.set_partition_spec(spec);
        } else if !decision.reason.is_empty() {
            // Keep the decision observable: EXPLAIN ANALYZE / PROFILE report
            // the reason whenever the plan falls back to serial execution.
            plan.parallel_fallback_reason = decision.reason;
        }
        plan
    }

    /// Replace the conservative partitioning configuration. This is intended
    /// for database setup, before the engine is shared by query pipelines.
    pub fn set_partitioning_config(&mut self, config: PartitioningConfig) {
        self.partitioning_planner = PartitioningPlanner::new(config);
    }

    pub fn partitioning_config(&self) -> &PartitioningConfig {
        self.partitioning_planner.config()
    }

    /// Apply heuristic optimization rules with caller-supplied iteration limit.
    fn apply_heuristic_with_max_iterations(
        &self,
        plan: ExecutionPlan,
        max_iterations: usize,
    ) -> OptimizeResult<ExecutionPlan> {
        // Interior mutability via AtomicUsize: set_max_iterations does not need &mut self.
        self.heuristic_batch.set_max_iterations(max_iterations);

        let root = match plan.root.clone() {
            Some(root) => root,
            None => return Ok(plan),
        };
        let result = self
            .heuristic_batch
            .optimize(root)
            .map_err(|e| OptimizeError::HeuristicFailed(e.to_string()))?;
        if let Ok(mut guard) = self.last_batch_statistics.lock() {
            *guard = result.batch_statistics.clone();
        }
        let mut new_plan = plan;
        new_plan.set_root(result.optimized_plan);
        Ok(new_plan)
    }

    /// Apply cost-based optimization strategies.
    ///
    /// Currently performs:
    /// - Subquery unnesting: PatternApply → HashInnerJoin when cost-beneficial
    /// - Join order optimization: reorder joins based on estimated costs
    /// - Index selection: ScanVertices → IndexScan when an index is cheaper
    /// - Sort + Limit → TopN conversion (residual patterns, cost-based)
    /// - Aggregate strategy selection (decision notes)
    /// - Per-node row estimate collection for `estimated_rows` writeback
    /// - Expression precomputation decisions (decision notes)
    fn apply_cost_based(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;
        let stats = StatsView::new(&self.stats_manager, space);

        // Phase 1: Subquery unnesting (PatternApply → HashInnerJoin)
        let root_clone = plan.root.clone();
        if let Some(ref root) = root_clone {
            let mut notes = Vec::new();
            let rewritten = self.unnest_subqueries(root, &stats, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 2: Join order optimization (extract tables/conditions, reorder)
        if let Some(ref root) = plan.root.clone() {
            let mut notes = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins(
                    root,
                    &stats,
                    &self.cost_calculator,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 3: Cost-based index selection (ScanVertices → IndexScan)
        if let Some(ref root) = plan.root.clone() {
            let selector = IndexSelector::new(
                self.cost_calculator.clone(),
                self.selectivity_estimator.clone(),
            );
            let mut notes = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::index_selection::rewrite_index_scans(
                    root,
                    &selector,
                    &self.stats_manager,
                    space,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 4: Sort + Limit → TopN conversion (residual patterns)
        if let Some(ref root) = plan.root.clone() {
            let optimizer = SortEliminationOptimizer::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits(
                    root,
                    &optimizer,
                    &stats,
                    &self.selectivity_estimator,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 5: Aggregate strategy selection (decision notes; the
        // strategy is consumed by the physical planner via the notes).
        if let Some(ref root) = plan.root.clone() {
            let selector = AggregateStrategySelector::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten = self.select_aggregate_strategies(root, &stats, &selector, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 6: Collect per-node row estimates for estimated_rows writeback.
        if let Some(ref root) = plan.root.clone() {
            plan.row_estimates =
                crate::query::optimizer::cost_based::row_estimates::collect_node_row_estimates(
                    root,
                    &stats,
                    &self.selectivity_estimator,
                );
        }

        // Phase 7: Expression precomputation decisions (note-only; EXPLAIN
        // observability for expressions worth precomputing).
        if let Some(ref root) = plan.root.clone() {
            let optimizer = crate::query::optimizer::cost_based::expression_precomputation::ExpressionPrecomputationOptimizer::new(self.cost_calculator.clone());
            let notes =
                crate::query::optimizer::cost_based::precomputation_wiring::collect_precompute_notes(
                    root,
                    &optimizer,
                );
            plan.cbo_notes.extend(notes);
        }

        Ok(plan)
    }

    /// Walk the plan and record the aggregation strategy decision for every
    /// aggregate node as a CBO note (cost-driven, observable in EXPLAIN).
    fn select_aggregate_strategies(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        selector: &AggregateStrategySelector,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use crate::query::optimizer::cost_based::row_estimates::estimate_node_output_rows;
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
        use PlanNodeEnum::*;

        if let Aggregate(aggregate) = node {
            let input_rows =
                estimate_node_output_rows(aggregate.input(), stats, &self.selectivity_estimator);
            let context = AggregateContext {
                input_rows,
                group_keys: aggregate.group_keys().to_vec(),
                agg_function_count: aggregate.aggregation_functions().len(),
                memory_limit: 0,
                input_is_sorted: false,
                sort_keys_match_group_keys: false,
                is_deterministic: true,
                complexity_score: 0,
                table_name: None,
            };
            let decision = selector.select_strategy(stats.space().unwrap_or(""), &context);
            notes.push(format!(
                "aggregate: strategy={:?} (reason={:?}, est_rows={})",
                decision.strategy, decision.reason, decision.estimated_output_rows
            ));
        }

        // Recursively rewrite children.
        let mut closure =
            |child: &PlanNodeEnum| self.select_aggregate_strategies(child, stats, selector, notes);
        crate::query::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
    }

    /// Recursively walk the plan tree and rewrite PatternApply → HashInnerJoin
    /// when the subquery unnesting optimizer determines it is beneficial.
    fn unnest_subqueries(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use PlanNodeEnum::*;

        // Try PatternApply unnesting at this level first.
        if let PatternApply(apply) = node {
            let analysis = self.batch_plan_analyzer.analyze(node);
            if let UnnestDecision::ShouldUnnest { ref reason, .. } = self
                .subquery_unnesting_optimizer
                .should_unnest(apply, &analysis, stats)
            {
                log::debug!(
                    "CBO: unnesting PatternApply -> HashInnerJoin ({:?})",
                    reason
                );
                notes.push(format!("unnest pattern_apply -> hash_join ({:?})", reason));
                if let Ok(join) = self.subquery_unnesting_optimizer.unnest(apply.clone()) {
                    return self.unnest_subqueries(&join, stats, notes);
                }
            }
        }

        // Recursively rewrite children for the most common node types.
        // Unsupported variants fall through to the catch-all and are returned
        // unchanged (their subtrees are not traversed for unnesting).
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
        macro_rules! rewrite_single {
            ($n:expr) => {{
                let mut cloned = $n.clone();
                let new_input = self.unnest_subqueries(cloned.input(), stats, notes);
                cloned.set_input(new_input);
                cloned
            }};
        }
        macro_rules! rewrite_binary {
            ($n:expr) => {{
                let mut cloned = $n.clone();
                let new_left = self.unnest_subqueries(cloned.left_input(), stats, notes);
                let new_right = self.unnest_subqueries(cloned.right_input(), stats, notes);
                cloned.set_left_input(new_left);
                cloned.set_right_input(new_right);
                cloned
            }};
        }

        match node {
            // Single-input operation nodes
            Project(n) => Project(rewrite_single!(n)),
            Filter(n) => Filter(rewrite_single!(n)),
            Sort(n) => Sort(rewrite_single!(n)),
            Limit(n) => Limit(rewrite_single!(n)),
            TopN(n) => TopN(rewrite_single!(n)),
            Sample(n) => Sample(rewrite_single!(n)),
            Dedup(n) => Dedup(rewrite_single!(n)),
            Aggregate(n) => Aggregate(rewrite_single!(n)),
            Window(n) => Window(rewrite_single!(n)),

            // Binary join nodes
            InnerJoin(n) => InnerJoin(rewrite_binary!(n)),
            LeftJoin(n) => LeftJoin(rewrite_binary!(n)),
            RightJoin(n) => RightJoin(rewrite_binary!(n)),
            CrossJoin(n) => CrossJoin(rewrite_binary!(n)),
            FullOuterJoin(n) => FullOuterJoin(rewrite_binary!(n)),
            SemiJoin(n) => SemiJoin(rewrite_binary!(n)),
            HashInnerJoin(n) => HashInnerJoin(rewrite_binary!(n)),
            HashLeftJoin(n) => HashLeftJoin(rewrite_binary!(n)),

            // PatternApply: unnesting was attempted above; if we reach here
            // the decision was to keep it, so rewrite the left child (the
            // main data pipeline). The right child (subquery pattern) is
            // typically a leaf scan and is left unchanged.
            PatternApply(n) => PatternApply(rewrite_single!(n)),

            // Leaf / unsupported nodes: return unchanged.
            _ => node.clone(),
        }
    }

    /// Get the heuristic batch optimizer
    pub fn heuristic_batch(&self) -> &BatchOptimizer {
        &self.heuristic_batch
    }

    /// Get the batch statistics of the last heuristic optimization (for EXPLAIN diagnostics)
    pub fn last_batch_statistics(&self) -> Vec<(OptimizationBatch, BatchStatistics)> {
        match self.last_batch_statistics.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

use crate::query::optimizer::error::{OptimizeError, OptimizeResult};

impl Default for OptimizerEngine {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_engine_creation() {
        let _engine = OptimizerEngine::default();
    }

    #[test]
    fn test_optimizer_engine_with_config() {
        let config = CostModelConfig::for_ssd();
        let _engine = OptimizerEngine::new(config);
    }

    #[test]
    fn test_optimizer_engine_configuration() {
        let mut engine = OptimizerEngine::default();

        // Test enable/disable heuristic
        engine.set_enable_heuristic(false);
        assert!(!engine.enable_heuristic);

        engine.set_enable_heuristic(true);
        assert!(engine.enable_heuristic);
    }

    #[test]
    fn test_optimizer_engine_max_iterations() {
        let mut engine = OptimizerEngine::default();

        engine.set_max_heuristic_iterations(50);
        assert_eq!(engine.max_heuristic_iterations, 50);
    }

    #[test]
    fn cost_based_phases_rewrite_and_emit_notes() {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::core::types::{Index, IndexStatus, IndexType};
        use crate::core::Value;
        use crate::query::optimizer::stats::TagStatistics;
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{
            LimitNode, SortItem, SortNode,
        };
        use std::sync::Arc;

        let engine = OptimizerEngine::default();

        // Register index + vertex statistics for the tag so cost-based
        // index selection and the row estimates are data-driven.
        engine.stats_manager().register_tag_indexes(
            "test",
            "person",
            7,
            vec![Index {
                id: 3,
                name: "idx_person_name".to_string(),
                space_id: 1,
                schema_name: "person".to_string(),
                fields: Vec::new(),
                properties: vec!["name".to_string()],
                index_type: IndexType::TagIndex,
                status: IndexStatus::Active,
                is_unique: false,
                comment: None,
                covering: false,
                partial_condition: None,
            }],
        );
        let mut tag_stats = TagStatistics::new("person".to_string());
        tag_stats.vertex_count = 10_000;
        engine.stats_manager().update_tag_stats("test", tag_stats);

        // Build: Limit -> Sort -> Filter(ScanVertices(person, name = 'alice')).
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        let context = Arc::new(ExpressionAnalysisContext::new());
        let predicate = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "name".to_string(),
            }),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::String("alice".into()))),
        };
        let id = context.register_expression(ExpressionMeta::new(predicate));
        let filter = FilterNode::new(
            PlanNodeEnum::ScanVertices(scan),
            ContextualExpression::new(id, context),
        )
        .expect("filter should build");
        let sort = SortNode::new(
            PlanNodeEnum::Filter(filter),
            vec![SortItem::column_asc("n.name".to_string())],
        )
        .expect("sort should build");
        let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::Limit(limit)));

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");

        // The heuristic phase converts Limit(offset=0) -> Sort to TopN, so
        // the plan must contain a TopN and an IndexScan for the predicate.
        let root = optimized.root.as_ref().expect("root should exist");
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::TopN(_))),
            "expected TopN in optimized plan"
        );
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
            "expected IndexScan in optimized plan"
        );

        // Decision notes and row estimates must be produced.
        assert!(optimized
            .cbo_notes
            .iter()
            .any(|note| note.starts_with("index:")));
        assert!(!optimized.row_estimates.is_empty());

        // The filter remains above the index scan (residual predicate).
        assert!(contains_variant(root, &|node| matches!(
            node,
            PlanNodeEnum::Filter(_)
        )));
    }

    fn contains_variant(node: &PlanNodeEnum, predicate: &dyn Fn(&PlanNodeEnum) -> bool) -> bool {
        if predicate(node) {
            return true;
        }
        node.children()
            .iter()
            .any(|child| contains_variant(child, predicate))
    }

    #[test]
    fn precompute_notes_emitted_for_reused_expressions() {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::core::types::operators::BinaryOperator;
        use crate::core::Value;
        use crate::core::YieldColumn;
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
        use std::sync::Arc;

        let mut engine = OptimizerEngine::default();
        // Keep the plan shape untouched so duplicate projection columns
        // survive (heuristic dedup would collapse them).
        engine.set_enable_heuristic(false);

        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());

        // (a + b) * 2: complex enough to clear the precomputation cost floor.
        let expr = Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::Variable("a".to_string())),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Variable("b".to_string())),
            }),
            op: BinaryOperator::Multiply,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let context = Arc::new(ExpressionAnalysisContext::new());
        let id = context.register_expression(ExpressionMeta::new(expr));
        let contextual = ContextualExpression::new(id, context);

        // The same expression is referenced by three projection columns.
        let columns: Vec<YieldColumn> = (0..3)
            .map(|i| YieldColumn {
                expression: contextual.clone(),
                alias: format!("c{}", i),
                is_matched: false,
            })
            .collect();
        let project = ProjectNode::new(PlanNodeEnum::ScanVertices(scan), columns)
            .expect("project should build");
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::Project(project)));

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");

        assert!(
            optimized
                .cbo_notes
                .iter()
                .any(|note| note.starts_with("precompute:")),
            "expected precompute decision notes, got: {:?}",
            optimized.cbo_notes
        );
    }
}
