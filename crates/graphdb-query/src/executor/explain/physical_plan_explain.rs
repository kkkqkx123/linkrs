//! PhysicalPlan → PlanDescription conversion for EXPLAIN.
//!
//! Step 2.2: The EXPLAIN statement now uses the arena PhysicalPlan as its
//! data source instead of the old ExecutionPlan / PlanNodeEnum tree.
//! This function walks the PhysicalPlan's operator arena and fragment DAG
//! to produce a PlanDescription compatible with the existing formatters.

use std::collections::{HashMap, HashSet};

use crate::executor::streaming::plan::types::{
    FragmentGraph, FragmentId, PhysicalOperatorId, PhysicalPlan,
};
use crate::planning::plan::explain::{Pair, PlanDescription, PlanNodeDescription};

/// Build a [`PlanDescription`] from an arena [`PhysicalPlan`].
///
/// Walks the fragment DAG in topological order (producers before consumers)
/// and creates one [`PlanNodeDescription`] per physical operator.
pub fn physical_plan_to_plan_description(plan: &PhysicalPlan) -> PlanDescription {
    let mut desc = PlanDescription::new();
    desc.requested_workers = 1;
    desc.parallel_fallback_reason = plan.parallel_fallback_reason.clone();
    desc.cbo_notes = plan.cbo_notes.clone();
    if let Some(spec) = plan.partition_spec() {
        let version = spec
            .layout_version()
            .map(|v| format!(", layout_version={v}"))
            .unwrap_or_default();
        desc.partition_spec_description = Some(match spec.strategy() {
            crate::planning::plan::PartitionStrategy::Range => {
                let ranges = spec
                    .ranges()
                    .iter()
                    .map(|r| format!("[{}..{})", r.start, r.end))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} partitioned into {} ranges [{}]{}",
                    spec.source(),
                    spec.partition_count(),
                    ranges,
                    version
                )
            }
            crate::planning::plan::PartitionStrategy::Hash { key } => {
                let key_desc = if key.is_empty() {
                    String::new()
                } else {
                    format!(" by key '{key}'")
                };
                format!(
                    "{} hash-partitioned into {} buckets{}{}",
                    spec.source(),
                    spec.partition_count(),
                    key_desc,
                    version
                )
            }
            crate::planning::plan::PartitionStrategy::RoundRobin => {
                format!(
                    "{} round-robin partitioned into {} buckets{}",
                    spec.source(),
                    spec.partition_count(),
                    version
                )
            }
        });
    }

    // Build a reverse dependency map: operator_id → list of operator ids that depend on it.
    // Also track which fragment each operator belongs to.
    let topo = topological_operator_order(&plan.fragments, plan);

    let mut operator_to_fragment: HashMap<PhysicalOperatorId, FragmentId> = HashMap::new();
    for frag in plan.fragments.fragments() {
        for op_id in &frag.operators {
            operator_to_fragment.insert(*op_id, frag.id);
        }
    }

    // parent_of[b] = a means "a depends on b" → b is a child of a in the output DAG.
    // We build this by following the fragment graph.
    let mut parent_of: HashMap<PhysicalOperatorId, Vec<PhysicalOperatorId>> = HashMap::new();

    // Within each fragment, operators form a pipeline (listed leaf→root).
    for frag in plan.fragments.fragments() {
        for (i, op_id) in frag.operators.iter().enumerate() {
            if i > 0 {
                // op[i] depends on op[i-1] within the same fragment
                parent_of
                    .entry(frag.operators[i - 1])
                    .or_default()
                    .push(*op_id);
            }
        }
        // The first operator in a fragment depends on the root operators of input fragments.
        if let Some(first_op) = frag.operators.first() {
            for input_fid in &frag.inputs {
                if let Some(input_frag) = plan.fragments.get(*input_fid) {
                    parent_of
                        .entry(input_frag.root_operator)
                        .or_default()
                        .push(*first_op);
                }
            }
        }
    }

    // Collect all operator IDs in topological order (producers first).
    let mut desc_ids: HashSet<PhysicalOperatorId> = HashSet::new();
    let mut ordered_ops: Vec<PhysicalOperatorId> = Vec::new();
    for &fid in &topo {
        if let Some(frag) = plan.fragments.get(fid) {
            for op_id in &frag.operators {
                if desc_ids.insert(*op_id) {
                    ordered_ops.push(*op_id);
                }
            }
        }
    }

    for &op_id in &ordered_ops {
        let op_spec = match plan.operator(op_id) {
            Some(s) => s,
            None => continue,
        };

        let mut pnd = PlanNodeDescription::new(op_spec.explain_name, op_id.0 as i64);

        // Extract description from properties
        let mut pairs = Vec::new();
        if let Some(est) = op_spec.estimated_cardinality {
            pairs.push(Pair::new("est_rows", est.to_string()));
        }
        if let Some(reason) = &op_spec.choice_reason {
            pairs.push(Pair::new("reason", reason.clone()));
        }
        // Constant folding applied on the source logical node.
        if op_spec.has_folded_expressions {
            pairs.push(Pair::new("folded", "true"));
        }

        let props = &op_spec.properties;
        if let crate::executor::streaming::plan::properties::Ordering::Sorted(orders) =
            &props.ordering
        {
            let order_str = orders
                .iter()
                .map(|o| {
                    format!(
                        "slot#{} {}",
                        o.slot,
                        if o.ascending { "ASC" } else { "DESC" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            if !order_str.is_empty() {
                pairs.push(Pair::new("ordering", order_str));
            }
        }

        if let crate::executor::streaming::plan::properties::Distribution::HashPartitioned(
            keys,
        ) = &props.distribution
        {
            if !keys.is_empty() {
                pairs.push(Pair::new(
                    "hash_slots",
                    keys.iter()
                        .map(|s| format!("slot#{}", s))
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
        }

        if props.pipeline_kind
            == crate::executor::streaming::plan::properties::PipelineKind::Blocking
        {
            pairs.push(Pair::new("blocking", "true"));
        }

        // PatternApply subquery keys: hash_keys evaluate against the outer
        // (left) layout, probe_keys against the subquery (right) layout.
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Apply(
            crate::executor::streaming::operators::spec::ApplySpec::PatternApply {
                hash_keys,
                probe_keys,
                anti,
            },
        ) = &op_spec.spec
        {
            if !hash_keys.is_empty() {
                pairs.push(Pair::new(
                    "hash_keys",
                    hash_keys
                        .iter()
                        .map(crate::core::types::expr::Expression::to_expression_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            if !probe_keys.is_empty() {
                pairs.push(Pair::new(
                    "probe_keys",
                    probe_keys
                        .iter()
                        .map(crate::core::types::expr::Expression::to_expression_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            if *anti {
                pairs.push(Pair::new("anti", "true"));
            }
        }

        // CorrelatedApply: the right subtree is re-executed per outer row.
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Apply(
            crate::executor::streaming::operators::spec::ApplySpec::CorrelatedApply {
                anti,
                ..
            },
        ) = &op_spec.spec
        {
            if *anti {
                pairs.push(Pair::new("anti", "true"));
            }
        }

        // SemiJoin (Mark-Join): surface the residual join condition and the
        // anti (NOT EXISTS) semantics.
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Join(
            crate::executor::streaming::operators::spec::JoinSpec::SemiJoin {
                join_condition,
                anti,
            },
        ) = &op_spec.spec
        {
            if let Some(condition) = join_condition {
                pairs.push(Pair::new(
                    "join_condition",
                    condition.to_expression_string(),
                ));
            }
            if *anti {
                pairs.push(Pair::new("anti", "true"));
            }
        }

        // Expression-level subqueries: Filter/Project/Assign hosts
        // surface the number of compiled subqueries as `subquery: N`.
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Unary(unary_spec) =
            &op_spec.spec
        {
            let count = unary_spec.subquery_runners().len();
            if count > 0 {
                pairs.push(Pair::new("subquery", count.to_string()));
            }
        }

        if let crate::executor::streaming::plan::properties::MemoryPolicy::Spillable {
            threshold,
        } = &props.memory_policy
        {
            pairs.push(Pair::new("spill_threshold", format!("{}", threshold)));
        }

        // Annotated ExpandAll hops surface their de-materialization mode.
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Graph(
            crate::executor::streaming::operators::spec::GraphSpec::ExpandAll {
                count_only,
                emit_raw_ids,
                ..
            },
        ) = &op_spec.spec
        {
            if *count_only {
                pairs.push(crate::planning::plan::explain::Pair::new(
                    "mode",
                    "count_only",
                ));
            } else if *emit_raw_ids {
                pairs.push(crate::planning::plan::explain::Pair::new(
                    "mode", "id_only",
                ));
            }
        }

        // Try to extract output column names from SourceSpec
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Source(src_spec) =
            &op_spec.spec
        {
            let col_names: Vec<String> = match src_spec {
                crate::executor::streaming::operators::spec::SourceSpec::ScanVertices { col_names, .. }
                | crate::executor::streaming::operators::spec::SourceSpec::StandaloneValues { col_names, .. }
                => col_names.clone(),
                crate::executor::streaming::operators::spec::SourceSpec::StorageScanVertices { col_names, .. } => col_names.clone(),
                crate::executor::streaming::operators::spec::SourceSpec::ScanEdges { col_names, .. } => col_names.clone(),
                crate::executor::streaming::operators::spec::SourceSpec::StorageScanEdges { col_names, .. } => col_names.clone(),
                _ => vec![],
            };
            if !col_names.is_empty() {
                pnd.output_var = col_names.join(", ");
            }
            // Surface the projected property list for storage scans and
            // graph-operator sources so the property-pruning optimizations
            // (EnrichScanSlotsWithFilterProps, typed GetVertices/GetNeighbors
            // pushdown) are observable in EXPLAIN output.
            let projected: Vec<String> = match src_spec {
                crate::executor::streaming::operators::spec::SourceSpec::StorageScanVertices { projected_properties, .. }
                | crate::executor::streaming::operators::spec::SourceSpec::StorageScanEdges { projected_properties, .. }
                | crate::executor::streaming::operators::spec::SourceSpec::GetVertices { projected_properties, .. }
                | crate::executor::streaming::operators::spec::SourceSpec::GetEdges { projected_properties, .. }
                | crate::executor::streaming::operators::spec::SourceSpec::GetNeighbors { projected_properties, .. }
                => projected_properties.clone(),
                _ => vec![],
            };
            if !projected.is_empty() {
                pairs.push(crate::planning::plan::explain::Pair::new(
                    "projected",
                    projected.join(","),
                ));
            }
        }

        if !pairs.is_empty() {
            pnd.description = Some(pairs);
        }

        // Dependencies
        if let Some(deps) = parent_of.get(&op_id) {
            let dep_ids: Vec<i64> = deps.iter().map(|d| d.0 as i64).collect();
            pnd.set_dependencies(dep_ids);
        }

        desc.add_node_desc(pnd);
    }

    desc
}

/// Return fragment IDs in topological order (sources first, root last).
fn topological_operator_order(fragments: &FragmentGraph, _plan: &PhysicalPlan) -> Vec<FragmentId> {
    let root = fragments.root();
    let mut order = Vec::new();
    let mut visited = HashSet::new();
    let mut stack = vec![root];

    while let Some(fid) = stack.pop() {
        if !visited.insert(fid) {
            continue;
        }
        if let Some(frag) = fragments.get(fid) {
            for &input in &frag.inputs {
                stack.push(input);
            }
            order.push(fid);
        }
    }

    order.reverse();
    order
}
