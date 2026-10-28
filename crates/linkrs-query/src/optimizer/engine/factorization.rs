use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::ExecutionPlan;

use super::OptimizerEngine;
use crate::optimizer::error::OptimizeResult;

impl OptimizerEngine {
    pub(super) fn apply_remove_factorization(
        &self,
        mut plan: ExecutionPlan,
    ) -> OptimizeResult<ExecutionPlan> {
        if let Some(logical) = plan.logical_plan.as_mut() {
            crate::optimizer::factorization::RemoveFactorizationRewriter::new()
                .rewrite(&mut logical.root)?;
            plan.cbo_notes
                .push("factorization: removed LogicalFlatten".to_string());
        }
        Ok(plan)
    }

    pub(super) fn apply_factorization(
        &self,
        mut plan: ExecutionPlan,
    ) -> OptimizeResult<ExecutionPlan> {
        if let Some(logical) = plan.logical_plan.as_mut() {
            let mut rewriter = crate::optimizer::factorization::FactorizationRewriter::new();
            rewriter.rewrite(&mut logical.root)?;
            let mut flattens = Vec::new();
            crate::planning::physical_mapper::PhysicalMapper::collect_flatten_positions(
                &logical.root,
                &mut flattens,
            );
            flattens.sort_unstable();
            flattens.dedup();
            for pos in &flattens {
                plan.cbo_notes.push(format!("Flatten(group={})", pos));
            }
            let mut skipped = rewriter.take_skipped_flat_groups();
            skipped.sort_unstable();
            skipped.dedup();
            for pos in &skipped {
                if !flattens.contains(pos) {
                    plan.cbo_notes
                        .push(format!("factorization: flatten_noop_flat(group={})", pos));
                }
            }
            if !flattens.is_empty() {
                plan.cbo_notes
                    .push(format!("factorization: flatten_total={}", flattens.len()));
                if let Some(ref ms) = self.metrics_stats {
                    ms.add_value_with_amount(
                        linkrs_metrics::MetricType::FactorizationFlattenTotal,
                        flattens.len() as u64,
                    );
                }
            }
            if plan
                .cbo_notes
                .iter()
                .all(|n| !n.starts_with("Flatten(group="))
            {
                plan.cbo_notes
                    .push("factorization: re-inserted LogicalFlatten".to_string());
            }
        }
        Ok(plan)
    }

    pub(super) fn apply_intersect_to_join_rewrite(
        &self,
        mut plan: ExecutionPlan,
        space: Option<&str>,
    ) -> ExecutionPlan {
        let stats = crate::optimizer::stats::StatsView::new(&self.stats_manager, space);
        if let Some(logical) = plan.logical_plan.as_ref() {
            let mut root = logical.root.clone();
            let mut notes = Vec::new();
            let mut rewrite_count = 0u64;
            Self::rewrite_intersect_to_join(
                &mut root,
                &stats,
                &self.selectivity_estimator,
                &mut notes,
                &mut rewrite_count,
            );
            if rewrite_count > 0 {
                if Self::validate_factorized_invariant(&root) {
                    if let Some(logical) = plan.logical_plan.as_mut() {
                        logical.root = root;
                    }
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(format!(
                        "factorization: intersect_to_join_rewrite_total={}",
                        rewrite_count
                    ));
                    if let Some(ref ms) = self.metrics_stats {
                        ms.add_value_with_amount(
                            linkrs_metrics::MetricType::FactorizationFallbackTotal,
                            rewrite_count,
                        );
                    }
                } else {
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(
                        "factorization: intersect_to_join_rewrite_total=0 \
                         (reverted, schema invariant violated)"
                            .to_string(),
                    );
                }
            } else if !notes.is_empty() {
                plan.cbo_notes.extend(notes);
            }
        }
        plan
    }

    const MAX_INTERSECT_REWRITE_BUILDS: usize = 8;

    fn rewrite_intersect_to_join(
        node: &mut LogicalNodeEnum,
        stats: &crate::optimizer::stats::StatsView,
        selectivity: &crate::optimizer::SelectivityEstimator,
        notes: &mut Vec<String>,
        rewrite_count: &mut u64,
    ) {
        for child in node.children_mut() {
            Self::rewrite_intersect_to_join(child, stats, selectivity, notes, rewrite_count);
        }

        let snapshot = node.clone();
        let LogicalNodeEnum::WcoIntersect(wco) = &snapshot else {
            return;
        };
        use crate::optimizer::cost_based::row_estimates::estimate_node_output_rows_logical;
        use crate::planning::join_order::cost_model::CostModel;

        let probe_rows = estimate_node_output_rows_logical(wco.probe_side(), stats, selectivity);
        let mut build_costs = Vec::with_capacity(wco.num_builds());
        let mut total_build_cost = 0u64;
        for build in wco.build_sides() {
            let rows = estimate_node_output_rows_logical(build, stats, selectivity);
            total_build_cost = total_build_cost.saturating_add(rows);
            build_costs.push(rows);
        }
        let output_rows = probe_rows.min(total_build_cost).max(1);
        let intersect_cost =
            CostModel::compute_intersect_cost(0, probe_rows, &build_costs, output_rows);
        let join_key_cardinality = probe_rows;
        let hash_cost = CostModel::compute_hash_join_cost(
            0,
            probe_rows,
            total_build_cost,
            join_key_cardinality,
        );
        if hash_cost >= intersect_cost {
            return;
        }
        if wco.num_builds() > Self::MAX_INTERSECT_REWRITE_BUILDS {
            notes.push(format!(
                "factorization: WcoIntersect kept (intersect_cost={}, hash_cost={}, \
                 reason: {} build sides exceed rewrite limit {})",
                intersect_cost,
                hash_cost,
                wco.num_builds(),
                Self::MAX_INTERSECT_REWRITE_BUILDS,
            ));
            return;
        }
        *node = Self::build_join_chain_from_intersect(wco);
        *rewrite_count += 1;
        notes.push(format!(
            "factorization: WcoIntersect fallback to HashJoin \
             (intersect_cost={}, hash_cost={}, reason: hash join cheaper)",
            intersect_cost, hash_cost,
        ));
    }

    fn build_join_chain_from_intersect(
        wco: &crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode,
    ) -> LogicalNodeEnum {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;

        let intersect_key = wco.intersect_key().clone();
        let mut acc = wco.probe_side().clone();
        for build in wco.build_sides() {
            let mut col_names = acc.col_names().to_vec();
            if let Some(name) = intersect_key.as_variable() {
                if !col_names.iter().any(|c| c == &name) {
                    col_names.push(name);
                }
            }
            for col in build.col_names() {
                if !col_names.contains(col) {
                    col_names.push(col.clone());
                }
            }
            let join = LogicalInnerJoinNode {
                id: next_node_id(),
                left: Box::new(acc.clone()),
                right: Box::new(build.clone()),
                hash_keys: vec![intersect_key.clone()],
                probe_keys: vec![intersect_key.clone()],
                recommended_algorithm: None,
                output_var: wco.output_var().map(|s| s.to_string()),
                col_names,
                column_types: vec![],
            };
            acc = LogicalNodeEnum::InnerJoin(join);
        }
        acc
    }

    pub(super) fn validate_factorized_invariant(root: &LogicalNodeEnum) -> bool {
        Self::compute_schema_tree(root).is_ok()
    }

    fn compute_schema_tree(
        node: &LogicalNodeEnum,
    ) -> Result<
        crate::planning::plan::factorization::FactorizedSchema,
        crate::planning::plan::factorization::FactorizationError,
    > {
        use crate::planning::plan::factorization::FactorizedSchemaCompute;
        let child_schemas: Vec<_> = node
            .children_ref()
            .iter()
            .map(|child| Self::compute_schema_tree(child))
            .collect::<Result<Vec<_>, crate::planning::plan::factorization::FactorizationError>>(
            )?;
        let mut owned = node.clone();
        owned.compute_factorized_schema(&child_schemas)
    }
}
