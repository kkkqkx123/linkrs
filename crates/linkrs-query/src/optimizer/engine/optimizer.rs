use crate::optimizer::cost_based::subquery_unnesting::UnnestDecision;
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
        // Opt-stage timing gate for cold-planning diagnosis. Default off;
        // set LINKRS_OPT_STAGES=1 to print per-stage times to stderr.
        // The marks below stay aligned with the stage order so a single
        // probe run attributes the whole optimize budget.
        let stage_timing = std::env::var("LINKRS_OPT_STAGES").is_ok();
        let mut stage_start = stage_timing.then(std::time::Instant::now);
        let mut current_plan = plan;

        self.maybe_apply_feedback();
        Self::mark_opt_stage("feedback", &mut stage_start, stage_timing);

        current_plan = self.ensure_logical_plan(current_plan);
        Self::mark_opt_stage("ensure_logical", &mut stage_start, stage_timing);

        // Fixed-shape short-circuit: point lookups and one-hop traversals
        // lower to linear chains whose full rule batches and cost model are
        // provably redundant. The gate admits only that grammar; everything
        // else falls through to the full path below.
        // Diagnostic escape hatch: LINKRS_NO_FAST_SHAPE=1 forces the full
        // path for before/after measurement. Default off.
        let fast = self.enable_heuristic
            && std::env::var("LINKRS_NO_FAST_SHAPE").is_err()
            && current_plan.root.as_ref().is_some_and(Self::is_fast_shape);
        Self::mark_opt_stage("fast_shape_gate", &mut stage_start, stage_timing);
        if fast {
            return self.optimize_fast_shape(
                current_plan,
                space,
                layout,
                &mut stage_start,
                stage_timing,
            );
        }

        self.optimize_full_shape(current_plan, space, layout, &mut stage_start, stage_timing)
    }

    /// Full optimization path: every rule batch and cost pass. The fast
    /// path skips a provably redundant subset of these stages on fixed
    /// shapes; this function is the reference the fast path is tested
    /// against.
    pub(crate) fn optimize_full_shape(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
        stage_start: &mut Option<std::time::Instant>,
        stage_timing: bool,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;

        current_plan = self.apply_remove_factorization(current_plan)?;
        Self::mark_opt_stage("remove_factorization", stage_start, stage_timing);

        if self.enable_heuristic {
            log::debug!("Starting logical heuristic optimization");
            current_plan = self.apply_logical_heuristic(current_plan)?;
            log::debug!("Logical heuristic optimization completed successfully");
        }
        Self::mark_opt_stage("logical_heuristic", stage_start, stage_timing);

        log::debug!("Starting cost-based optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Cost-based optimization completed successfully");
        Self::mark_opt_stage("cost_based", stage_start, stage_timing);

        current_plan = self.apply_factorization(current_plan)?;
        Self::mark_opt_stage("factorization", stage_start, stage_timing);

        current_plan = self.apply_intersect_to_join_rewrite(current_plan, space);
        Self::mark_opt_stage("intersect_to_join", stage_start, stage_timing);

        current_plan = self.apply_physical_mapping(current_plan)?;
        Self::mark_opt_stage("physical_mapping", stage_start, stage_timing);

        if self.enable_heuristic {
            log::debug!("Starting physical heuristic optimization");
            current_plan =
                self.apply_physical_heuristic(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Physical heuristic optimization completed successfully");
        }
        Self::mark_opt_stage("physical_heuristic", stage_start, stage_timing);

        // Partitioning is decided on the final operator tree: the partitioned
        // execution paths require the de-materialized hop shape the physical
        // heuristic produces, so an earlier decision would only ever see an
        // unpartitionable plan. The spec still reaches the physical build and
        // the plan-cache fingerprint, which both run after the optimizer.
        current_plan = self.apply_partitioning_selection(current_plan, space, layout);
        Self::mark_opt_stage("partitioning", stage_start, stage_timing);

        // Row estimates are collected last so the per-node map keys match the
        // final plan ids: physical mapping and the physical heuristic rebuild
        // scan/filter nodes with fresh ids after cost-based decisions, and a
        // mid-pipeline map misses them (scan est_rows=1, filter est_rows=0.5
        // fallbacks in EXPLAIN). Decisions already consumed direct estimator
        // calls, so only the writeback map and feedback baselines move here.
        {
            let stats = StatsView::new(&self.stats_manager, space);
            self.apply_row_estimates(&mut current_plan, &stats);
        }
        Self::mark_opt_stage("row_estimates", stage_start, stage_timing);

        Ok(current_plan)
    }

    /// Emit one opt-stage timing mark when the `LINKRS_OPT_STAGES` gate is
    /// set, then re-arm the clock for the next stage. Silent otherwise.
    fn mark_opt_stage(name: &str, start: &mut Option<std::time::Instant>, enabled: bool) {
        if !enabled {
            return;
        }
        if let Some(s) = start.take() {
            eprintln!("OPT_STAGE {name} took {:?}", s.elapsed());
        }
        *start = enabled.then(std::time::Instant::now);
    }

    fn ensure_logical_plan(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        // Reverse conversion is the single bridge for physical-only
        // statements, which stay flat by design. Factorized planners emit
        // the logical tree natively, so a failure here records the reason
        // and keeps flat execution instead of masking it.
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
                        plan.diagnostics.notes.push(msg.clone());
                        plan.diagnostics.notes.push(format!(
                            "factorization: logical_plan_fallback_total=1 (node={})",
                            node_type
                        ));
                        if let Some(ref ms) = self.metrics_stats {
                            ms.add_value(linkrs_metrics::MetricType::FactorizationFallbackTotal);
                        }
                        if plan.diagnostics.fallback_reason.is_empty() {
                            plan.diagnostics.fallback_reason = msg;
                        } else {
                            plan.diagnostics.fallback_reason.push_str("; ");
                            plan.diagnostics.fallback_reason.push_str(&msg);
                        }
                    }
                }
            }
        }
        plan
    }

    fn apply_physical_mapping(&self, mut plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        let needs_mapping = plan.logical_plan.as_ref().is_some_and(|logical| {
            crate::planning::physical_mapper::PhysicalMapper::needs_physical_mapping(&logical.root)
        });
        if !needs_mapping {
            return Ok(plan);
        }
        if let Some(logical) = plan.logical_plan.as_ref() {
            if let Some(root) = plan.root.take() {
                let mapped =
                    crate::planning::physical_mapper::PhysicalMapper::map(logical.root.clone());
                match crate::planning::physical_mapper::PhysicalMapper::merge_physical_hints(
                    mapped, root,
                ) {
                    Ok(merged) => {
                        log::debug!("PhysicalMapping: merged logical mapping into physical plan");
                        plan.set_root(merged);
                    }
                    Err(err) => {
                        return Err(OptimizeError::PlanOptimizationError(err.to_string()));
                    }
                }
            }
        }
        Ok(plan)
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
            if plan.diagnostics.fallback_reason.is_empty() {
                plan.diagnostics.fallback_reason = decision.reason;
            } else {
                plan.diagnostics.fallback_reason.push_str("; ");
                plan.diagnostics.fallback_reason.push_str(&decision.reason);
            }
        }
        plan
    }

    fn apply_logical_heuristic(&self, mut plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        // Logical layer runs before cost decisions on the logical tree and
        // currently holds no built-in rules by design. Skip the call entirely
        // when the registry is empty so queries do not pay a no-op pass.
        // Rule injection stays closed: tuning goes through configuration filtering, not through
        // added rules. The active built-in rules live in the physical layer
        // below, so the two calls are not overlapping passes to be merged.
        if self.logical_heuristic.is_empty() {
            return Ok(plan);
        }
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

    /// Fixed-shape gate for the fast optimization path: linear chains of
    /// Project/Filter over a scan, with at most one ExpandAll hop.
    ///
    /// Point lookups and one-hop traversals lower to this grammar. Every
    /// omitted rule and skipped cost pass requires a node type outside it
    /// (joins, set operations, sorts, limits, aggregates, subqueries,
    /// traversals), so anything outside the grammar transparently runs the
    /// full path and the two paths can never disagree on a gated shape.
    pub(crate) fn is_fast_shape(root: &PlanNodeEnum) -> bool {
        let mut expand_hops = 0u32;
        Self::fast_shape_chain(root, &mut expand_hops)
    }

    fn fast_shape_chain(node: &PlanNodeEnum, expand_hops: &mut u32) -> bool {
        match node {
            PlanNodeEnum::Project(_) | PlanNodeEnum::Filter(_) => {}
            PlanNodeEnum::ExpandAll(_) => {
                *expand_hops += 1;
                if *expand_hops > 1 {
                    return false;
                }
            }
            PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::IndexScan(_) => return true,
            _ => return false,
        }
        let children = node.children();
        if children.len() != 1 {
            return false;
        }
        Self::fast_shape_chain(children[0], expand_hops)
    }

    /// Fast optimization path for fixed shapes: the reduced rule batch plus
    /// only the cost passes that can fire on the gated grammar.
    ///
    /// Skipped passes (unnesting, join-order rewrite, TopN wiring,
    /// aggregate strategies, intersect-to-join) act solely on node types the
    /// gate excludes and push notes only when they rewrite, so on gated
    /// shapes skipping them is exactly identical. Index selection, the
    /// join-order keep-decision notes, precompute notes, feedback,
    /// partitioning, and row estimates all run unchanged.
    pub(crate) fn optimize_fast_shape(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
        stage_start: &mut Option<std::time::Instant>,
        stage_timing: bool,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;
        current_plan
            .diagnostics
            .notes
            .push("fast_shape: linear scan/expand chain takes the reduced rule batch".to_string());

        current_plan = self.apply_remove_factorization(current_plan)?;
        Self::mark_opt_stage("remove_factorization", stage_start, stage_timing);

        if self.enable_heuristic {
            current_plan = self.apply_logical_heuristic(current_plan)?;
        }
        Self::mark_opt_stage("logical_heuristic", stage_start, stage_timing);

        current_plan = self.apply_cost_based_fast(current_plan, space)?;
        Self::mark_opt_stage("cost_based_fast", stage_start, stage_timing);

        current_plan = self.apply_factorization(current_plan)?;
        Self::mark_opt_stage("factorization", stage_start, stage_timing);

        current_plan = self.apply_physical_mapping(current_plan)?;
        Self::mark_opt_stage("physical_mapping", stage_start, stage_timing);

        if self.enable_heuristic {
            current_plan =
                self.apply_fast_heuristic(current_plan, self.max_heuristic_iterations)?;
        }
        Self::mark_opt_stage("fast_heuristic", stage_start, stage_timing);

        current_plan = self.apply_partitioning_selection(current_plan, space, layout);
        Self::mark_opt_stage("partitioning", stage_start, stage_timing);

        {
            let stats = StatsView::new(&self.stats_manager, space);
            self.apply_row_estimates(&mut current_plan, &stats);
        }
        Self::mark_opt_stage("row_estimates", stage_start, stage_timing);

        Ok(current_plan)
    }

    /// Reduced cost model for fixed shapes: index selection (the one pass
    /// that can fire on gated chains), the join-order keep-decision notes,
    /// and the notes-only precompute scan. Unnesting, join rewrites, TopN
    /// wiring, and aggregate strategies need absent node types.
    fn apply_cost_based_fast(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;
        let stats = StatsView::new(&self.stats_manager, space);
        if plan.logical_plan().is_some() {
            self.apply_join_order_logical(&stats, space, &mut plan);
            self.apply_index_selection_logical(space, &mut plan);
        } else if let Some(root) = plan.root.take() {
            use crate::optimizer::cost_based::index_selection::rewrite_index_scans;
            let selector = IndexSelector::new(
                self.cost_calculator_for(space),
                self.selectivity_estimator.clone(),
            );
            let mut notes = Vec::new();
            let rewritten =
                rewrite_index_scans(&root, &selector, &self.stats_manager, space, &mut notes);
            plan.set_root(rewritten);
            plan.diagnostics.notes.extend(notes);
        }
        self.apply_precompute_notes(space, &mut plan);
        Ok(plan)
    }

    /// Reduced-batch physical heuristic for fixed shapes. Records batch
    /// statistics exactly like the full path so probes stay comparable.
    fn apply_fast_heuristic(
        &self,
        mut plan: ExecutionPlan,
        max_iterations: usize,
    ) -> OptimizeResult<ExecutionPlan> {
        self.fast_heuristic.set_max_iterations(max_iterations);

        let Some(root) = plan.root.take() else {
            return Ok(plan);
        };
        let result = self
            .fast_heuristic
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

        // Dual path is intentional: statements that stay physical-only by
        // design (algorithm, fulltext, vector nodes without a logical
        // counterpart) keep flat execution through the physical path, while
        // factorized statements use the logical path. The two paths share
        // the underlying join, index, TopN, and aggregate primitives.
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
            let rewritten =
                crate::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins(
                    &root,
                    stats,
                    &calculator,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.diagnostics.notes.extend(notes);
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
            plan.diagnostics.notes.extend(notes);
        }

        self.apply_topn_wiring(space, plan, stats);

        if let Some(root) = plan.root.take() {
            let selector = AggregateStrategySelector::new(self.cost_calculator_for(space));
            let mut notes = Vec::new();
            let rewritten = self.select_aggregate_strategies(&root, stats, &selector, &mut notes);
            plan.set_root(rewritten);
            plan.diagnostics.notes.extend(notes);
        }

        self.apply_precompute_notes(space, plan);
        Ok(())
    }

    fn apply_unnesting(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(root) = plan.root.take() {
            let mut notes = Vec::new();
            let rewritten = self.unnest_subqueries(&root, stats, &mut notes);
            let rewritten = Self::apply_unnesting_heuristic_fallback(&rewritten, &mut notes);
            plan.set_root(rewritten);
            plan.diagnostics.notes.extend(notes);
        }
    }

    /// Post-CBO fallback for patterns the cost path explicitly kept.
    ///
    /// Only remaining `PatternApply` nodes are considered, so cost decisions
    /// are never preempted. A fallback unnest requires the same stat-free
    /// gates as the heuristic rule (simple bounded shape, no aggregation,
    /// fully deterministic) and is recorded as a heuristic note, keeping the
    /// decision source traceable per pattern.
    fn apply_unnesting_heuristic_fallback(
        node: &PlanNodeEnum,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use crate::optimizer::cost_based::subquery_unnesting::SubqueryUnnestingOptimizer;

        if let PlanNodeEnum::PatternApply(apply) = node {
            if SubqueryUnnestingOptimizer::is_stat_free_fallback_eligible(apply.right_input()) {
                if let Ok(joined) =
                    SubqueryUnnestingOptimizer::build_semi_join_from_pattern_apply(apply.clone())
                {
                    notes.push(
                        "unnest: heuristic fallback for cost-kept simple bounded subquery"
                            .to_string(),
                    );
                    return Self::apply_unnesting_heuristic_fallback(&joined, notes);
                }
            }
        }

        let mut closure =
            |child: &PlanNodeEnum| Self::apply_unnesting_heuristic_fallback(child, notes);
        crate::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
    }

    fn apply_unnesting_logical(&self, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::subquery_unnesting::unnest_pattern_applies_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let mut notes = Vec::new();
        let rewritten = unnest_pattern_applies_logical(logical.root(), &mut notes);
        plan.diagnostics.notes.extend(notes);
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

        if logical.should_skip_join_order_review() {
            if logical.join_order_hinted {
                plan.diagnostics.notes.push(
                    "join_order: kept planning order (user hint, reviewer skipped)".to_string(),
                );
            } else {
                plan.diagnostics.notes.push(
                    "join_order: kept planning order (statistics-backed, reviewer skipped)"
                        .to_string(),
                );
            }
            return;
        }

        let calculator = self.cost_calculator_for(space);
        let mut notes = Vec::new();
        let rewritten_logical =
            walk_and_optimize_joins_logical(logical.root(), stats, &calculator, &mut notes);
        plan.diagnostics.notes.extend(notes);

        let mut updated_logical = logical;
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        if let Some(root) = plan.root.take() {
            let mut scratch = Vec::new();
            let rewritten =
                crate::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins(
                    &root,
                    stats,
                    &calculator,
                    &mut scratch,
                );
            plan.set_root(rewritten);
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
        plan.diagnostics.notes.extend(notes);

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
        plan.diagnostics.notes.extend(notes);
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
        plan.diagnostics.notes.extend(notes);
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
            plan.diagnostics.notes.extend(notes);
        }
    }

    fn apply_row_estimates(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(root) = plan.root.as_ref() {
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
            plan.diagnostics.notes.extend(notes);
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
            notes.push(crate::optimizer::cost_based::describe_aggregate_strategy(
                stats.space().unwrap_or(""),
                aggregate.group_keys().to_vec(),
                aggregate.aggregation_functions().len(),
                input_rows,
                selector,
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

        let mut closure = |child: &PlanNodeEnum| self.unnest_subqueries(child, stats, notes);
        crate::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
    }
}
