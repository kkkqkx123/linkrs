use crate::optimizer::cost_based::subquery_unnesting::UnnestDecision;
use crate::optimizer::cost_based::AggregateContext;
use crate::optimizer::cost_based::{
    AggregateStrategySelector, IndexSelector, SortEliminationOptimizer,
};
use crate::optimizer::partitioning::PartitioningLayoutInfo;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::ExecutionPlan;
use crate::planning::plan::PlanNodeEnum;

use super::OptimizerEngine;
use crate::optimizer::error::{OptimizeError, OptimizeResult};

impl OptimizerEngine {
    pub fn optimize(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        self.optimize_with_layout(plan, space, &PartitioningLayoutInfo::default())
    }

    pub fn optimize_with_layout(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;

        self.maybe_apply_feedback();

        current_plan = self.ensure_logical_plan(current_plan);

        current_plan = self.apply_remove_factorization(current_plan)?;

        if self.enable_heuristic {
            log::debug!("Starting logical heuristic optimization");
            current_plan = self.apply_logical_heuristic(current_plan)?;
            log::debug!("Logical heuristic optimization completed successfully");
        }

        log::debug!("Starting cost-based optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Cost-based optimization completed successfully");

        current_plan = self.apply_factorization(current_plan)?;

        current_plan = self.apply_intersect_to_join_rewrite(current_plan, space);

        current_plan = self.apply_physical_mapping(current_plan);

        if self.enable_heuristic {
            log::debug!("Starting physical heuristic optimization");
            current_plan =
                self.apply_physical_heuristic(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Physical heuristic optimization completed successfully");
        }

        current_plan = self.apply_partitioning_selection(current_plan, space, layout);

        Ok(current_plan)
    }

    fn ensure_logical_plan(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if plan.logical_plan.is_none() {
            if let Some(root) = plan.root.as_ref() {
                let node_type = root.type_name().to_string();
                match crate::planning::plan::logical_plan::LogicalPlan::from_plan_node(root) {
                    Ok(logical) => {
                        plan.set_logical_plan(logical);
                    }
                    Err(e) => {
                        let msg = format!(
                            "logical_plan fallback failed: {} (factorization skipped, flat execution)",
                            e
                        );
                        log::warn!("LogicalPlan::from_plan_node fallback failed: {}", e);
                        plan.cbo_notes.push(msg.clone());
                        plan.cbo_notes.push(format!(
                            "factorization: logical_plan_fallback_total=1 (node={})",
                            node_type
                        ));
                        if let Some(ref ms) = self.metrics_stats {
                            ms.add_value(linkrs_metrics::MetricType::FactorizationFallbackTotal);
                        }
                        if plan.parallel_fallback_reason.is_empty() {
                            plan.parallel_fallback_reason = msg;
                        } else {
                            plan.parallel_fallback_reason.push_str("; ");
                            plan.parallel_fallback_reason.push_str(&msg);
                        }
                    }
                }
            }
        }
        plan
    }

    fn apply_physical_mapping(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        let needs_mapping = plan.logical_plan.as_ref().is_some_and(|logical| {
            crate::planning::physical_mapper::PhysicalMapper::needs_physical_mapping(&logical.root)
        });
        if !needs_mapping {
            return plan;
        }
        if let Some(logical) = plan.logical_plan.as_ref() {
            if let Some(root) = plan.root.take() {
                let mapped =
                    crate::planning::physical_mapper::PhysicalMapper::map(logical.root.clone());
                let (merged, notes) =
                    crate::planning::physical_mapper::PhysicalMapper::merge_physical_hints(
                        mapped, root,
                    );
                if notes.is_empty() {
                    log::debug!("PhysicalMapping: merged logical mapping into physical plan");
                } else {
                    let fallback_count = notes.len();
                    for note in &notes {
                        log::warn!("{note}");
                    }
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(format!(
                        "factorization: physical_mapping_fallback_total={}",
                        fallback_count
                    ));
                    if let Some(ref ms) = self.metrics_stats {
                        ms.add_value_with_amount(
                            linkrs_metrics::MetricType::FactorizationPhysicalMappingFallbackTotal,
                            fallback_count as u64,
                        );
                    }
                }
                plan.set_root(merged);
            }
        }
        plan
    }

    fn apply_partitioning_selection(
        &self,
        mut plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
    ) -> ExecutionPlan {
        if plan.partition_spec().is_some() {
            return plan;
        }
        let Some(root) = plan.root.as_ref() else {
            return plan;
        };
        let stats = StatsView::new(&self.stats_manager, space);
        let decision = self
            .partitioning_planner
            .decide_with_layout(root, &stats, layout);
        if let Some(spec) = decision.partition_spec {
            log::debug!("Selected partition layout: {}", decision.reason);
            plan.set_partition_spec(spec);
        } else if !decision.reason.is_empty() {
            if plan.parallel_fallback_reason.is_empty() {
                plan.parallel_fallback_reason = decision.reason;
            } else {
                plan.parallel_fallback_reason.push_str("; ");
                plan.parallel_fallback_reason.push_str(&decision.reason);
            }
        }
        plan
    }

    fn apply_logical_heuristic(&self, mut plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        self.logical_heuristic
            .set_max_iterations(self.max_heuristic_iterations);
        if let Some(logical) = plan.logical_plan.as_mut() {
            self.logical_heuristic.optimize(&mut logical.root)?;
        }
        Ok(plan)
    }

    fn apply_physical_heuristic(
        &self,
        mut plan: ExecutionPlan,
        max_iterations: usize,
    ) -> OptimizeResult<ExecutionPlan> {
        self.physical_heuristic.set_max_iterations(max_iterations);

        let Some(root) = plan.root.take() else {
            return Ok(plan);
        };
        let result = self
            .physical_heuristic
            .optimize(root)
            .map_err(|e| OptimizeError::HeuristicFailed(e.to_string()))?;
        if let Ok(mut guard) = self.last_batch_statistics.lock() {
            *guard = result.batch_statistics.clone();
        }
        plan.set_root(result.optimized_plan);
        Ok(plan)
    }

    fn apply_cost_based(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;
        let stats = StatsView::new(&self.stats_manager, space);

        if plan.logical_plan().is_some() {
            self.optimize_logical(&stats, space, &mut plan)?;
        } else {
            self.optimize_plan_nodes(&stats, space, &mut plan)?;
        }
        Ok(plan)
    }

    fn optimize_logical(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) -> OptimizeResult<()> {
        self.apply_unnesting(plan, stats);
        self.apply_unnesting_logical(plan);
        self.apply_join_order_logical(stats, space, plan);
        self.apply_index_selection_logical(space, plan);
        self.apply_topn_wiring(space, plan, stats);
        self.apply_topn_wiring_logical(stats, space, plan);
        self.apply_aggregate_strategy_logical(stats, space, plan);
        self.apply_row_estimates(plan, stats);
        self.apply_precompute_notes(space, plan);
        Ok(())
    }

    fn optimize_plan_nodes(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) -> OptimizeResult<()> {
        self.apply_unnesting(plan, stats);

        if let Some(root) = plan.root.take() {
            let calculator = self.cost_calculator_for(space);
            let mut notes = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::optimizer::cost_based::join_order_rewriter::
                walk_and_optimize_joins_with_decisions(
                    &root,
                    stats,
                    &calculator,
                    &mut notes,
                    &mut Some(&mut decisions),
                );
            plan.set_root(rewritten);
            plan.join_algorithms = decisions;
            plan.cbo_notes.extend(notes);
        }

        if let Some(root) = plan.root.take() {
            let selector = IndexSelector::new(
                self.cost_calculator_for(space),
                self.selectivity_estimator.clone(),
            );
            let mut notes = Vec::new();
            let rewritten = crate::optimizer::cost_based::index_selection::rewrite_index_scans(
                &root,
                &selector,
                &self.stats_manager,
                space,
                &mut notes,
            );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        self.apply_topn_wiring(space, plan, stats);

        if let Some(root) = plan.root.take() {
            let selector = AggregateStrategySelector::new(self.cost_calculator_for(space));
            let mut notes = Vec::new();
            let rewritten = self.select_aggregate_strategies(&root, stats, &selector, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        self.apply_row_estimates(plan, stats);
        self.apply_precompute_notes(space, plan);
        Ok(())
    }

    fn apply_unnesting(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(root) = plan.root.take() {
            let mut notes = Vec::new();
            let rewritten = self.unnest_subqueries(&root, stats, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }
    }

    fn apply_unnesting_logical(&self, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::subquery_unnesting::unnest_pattern_applies_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let mut notes = Vec::new();
        let rewritten = unnest_pattern_applies_logical(logical.root(), &mut notes);
        plan.cbo_notes.extend(notes);
        let mut updated_logical = logical;
        updated_logical.root = rewritten;
        plan.set_logical_plan(updated_logical);
    }

    pub(super) fn apply_join_order_logical(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) {
        use crate::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };

        if logical.join_order_hinted {
            plan.cbo_notes
                .push("join_order: kept planning order (user hint, reviewer skipped)".to_string());
            return;
        }

        let calculator = self.cost_calculator_for(space);
        let mut notes = Vec::new();
        let rewritten_logical =
            walk_and_optimize_joins_logical(logical.root(), stats, &calculator, &mut notes);
        plan.cbo_notes.extend(notes);

        let mut updated_logical = logical;
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        if let Some(root) = plan.root.take() {
            let mut scratch = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::optimizer::cost_based::join_order_rewriter::
                walk_and_optimize_joins_with_decisions(
                    &root,
                    stats,
                    &calculator,
                    &mut scratch,
                    &mut Some(&mut decisions),
                );
            plan.set_root(rewritten);
            plan.join_algorithms = decisions;
        }
    }

    fn apply_index_selection_logical(&self, space: Option<&str>, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::index_selection::rewrite_index_scans_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };

        let selector = IndexSelector::new(
            self.cost_calculator_for(space),
            self.selectivity_estimator.clone(),
        );

        let mut notes = Vec::new();
        let rewritten_logical = rewrite_index_scans_logical(
            logical.root(),
            &selector,
            &self.stats_manager,
            space,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);

        let mut updated_logical = logical;
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        if let Some(root) = plan.root.take() {
            let mut scratch = Vec::new();
            let rewritten = crate::optimizer::cost_based::index_selection::rewrite_index_scans(
                &root,
                &selector,
                &self.stats_manager,
                space,
                &mut scratch,
            );
            plan.set_root(rewritten);
        }
    }

    fn apply_aggregate_strategy_logical(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) {
        use crate::optimizer::cost_based::aggregate_strategy::walk_aggregate_strategies_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let selector = AggregateStrategySelector::new(self.cost_calculator_for(space));
        let mut notes = Vec::new();
        walk_aggregate_strategies_logical(
            logical.root(),
            stats,
            &selector,
            &self.selectivity_estimator,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);
    }

    fn apply_topn_wiring_logical(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) {
        use crate::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let optimizer = SortEliminationOptimizer::new(self.cost_calculator_for(space));
        let mut notes = Vec::new();
        let rewritten = rewrite_sort_with_limits_logical(
            logical.root(),
            &optimizer,
            stats,
            &self.selectivity_estimator,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);
        let mut updated_logical = logical;
        updated_logical.root = rewritten;
        plan.set_logical_plan(updated_logical);
    }

    fn apply_topn_wiring(&self, space: Option<&str>, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(root) = plan.root.take() {
            let optimizer = SortEliminationOptimizer::new(self.cost_calculator_for(space));
            let mut notes = Vec::new();
            let rewritten = crate::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits(
                &root,
                &optimizer,
                stats,
                &self.selectivity_estimator,
                &self.cardinality_feedback,
                &mut notes,
            );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }
    }

    fn apply_row_estimates(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(root) = plan.root.as_ref() {
            plan.row_estimates =
                crate::optimizer::cost_based::row_estimates::collect_node_row_estimates(
                    root,
                    stats,
                    &self.selectivity_estimator,
                );
            crate::optimizer::cost_based::row_estimates::register_plan_estimates(
                &self.cardinality_feedback,
                root,
                stats,
                &self.selectivity_estimator,
            );
        }
    }

    fn apply_precompute_notes(&self, space: Option<&str>, plan: &mut ExecutionPlan) {
        if let Some(root) = plan.root.as_ref() {
            let optimizer = crate::optimizer::cost_based::expression_precomputation::ExpressionPrecomputationOptimizer::new(self.cost_calculator_for(space));
            let notes =
                crate::optimizer::cost_based::precomputation_wiring::collect_precompute_notes(
                    root, &optimizer,
                );
            plan.cbo_notes.extend(notes);
        }
    }

    fn select_aggregate_strategies(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        selector: &AggregateStrategySelector,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use crate::optimizer::cost_based::row_estimates::estimate_node_output_rows;
        use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
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

        let mut closure =
            |child: &PlanNodeEnum| self.select_aggregate_strategies(child, stats, selector, notes);
        crate::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
    }

    fn unnest_subqueries(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use PlanNodeEnum::*;

        if let PatternApply(apply) = node {
            let analysis = self.batch_plan_analyzer.analyze(node);
            let advice = self.decision_feedback.advice(stats.space().unwrap_or(""));
            if let UnnestDecision::ShouldUnnest { ref reason, .. } =
                self.subquery_unnesting_optimizer.should_unnest(
                    apply,
                    &analysis,
                    stats,
                    &self.selectivity_estimator,
                    &self.cardinality_feedback,
                    &advice,
                )
            {
                log::debug!("CBO: unnesting PatternApply -> SemiJoin ({:?})", reason);
                notes.push(format!("unnest pattern_apply -> semi_join ({:?})", reason));
                if let Ok(join) = self.subquery_unnesting_optimizer.unnest(apply.clone()) {
                    return self.unnest_subqueries(&join, stats, notes);
                }
            }
        }

        use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
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
            Project(n) => Project(rewrite_single!(n)),
            Filter(n) => Filter(rewrite_single!(n)),
            Sort(n) => Sort(rewrite_single!(n)),
            Limit(n) => Limit(rewrite_single!(n)),
            TopN(n) => TopN(rewrite_single!(n)),
            Sample(n) => Sample(rewrite_single!(n)),
            Dedup(n) => Dedup(rewrite_single!(n)),
            Aggregate(n) => Aggregate(rewrite_single!(n)),
            Window(n) => Window(rewrite_single!(n)),

            InnerJoin(n) => InnerJoin(rewrite_binary!(n)),
            LeftJoin(n) => LeftJoin(rewrite_binary!(n)),
            RightJoin(n) => RightJoin(rewrite_binary!(n)),
            CrossJoin(n) => CrossJoin(rewrite_binary!(n)),
            FullOuterJoin(n) => FullOuterJoin(rewrite_binary!(n)),
            SemiJoin(n) => SemiJoin(rewrite_binary!(n)),

            PatternApply(n) => PatternApply(rewrite_single!(n)),
            CorrelatedApply(n) => CorrelatedApply(rewrite_single!(n)),

            _ => node.clone(),
        }
    }
}
