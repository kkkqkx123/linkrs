//! PhysicalPlan → PlanDescription conversion for EXPLAIN.
//!
//! Step 2.2: The EXPLAIN statement now uses the arena PhysicalPlan as its
//! data source instead of the old ExecutionPlan / PlanNodeEnum tree.
//! This function walks the PhysicalPlan's operator arena and fragment DAG
//! to produce a PlanDescription compatible with the existing formatters.

use std::collections::{HashMap, HashSet};

use crate::query::executor::streaming::plan::types::{
    FragmentGraph, FragmentId, PhysicalOperatorId, PhysicalPlan,
};
use crate::query::planning::plan::explain::{Pair, PlanDescription, PlanNodeDescription};

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
        let ranges = spec
            .ranges()
            .iter()
            .map(|r| format!("[{}..{})", r.start, r.end))
            .collect::<Vec<_>>()
            .join(", ");
        let version = spec
            .layout_version()
            .map(|v| format!(", layout_version={v}"))
            .unwrap_or_default();
        desc.partition_spec_description = Some(format!(
            "{} partitioned into {} ranges [{}]{}",
            spec.source(),
            spec.partition_count(),
            ranges,
            version
        ));
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

        let props = &op_spec.properties;
        if let crate::query::executor::streaming::plan::properties::Ordering::Sorted(orders) =
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

        if let crate::query::executor::streaming::plan::properties::Distribution::HashPartitioned(
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
            == crate::query::executor::streaming::plan::properties::PipelineKind::Blocking
        {
            pairs.push(Pair::new("blocking", "true"));
        }

        if let crate::query::executor::streaming::plan::properties::MemoryPolicy::Spillable {
            threshold,
        } = &props.memory_policy
        {
            pairs.push(Pair::new("spill_threshold", format!("{}", threshold)));
        }

        // Annotated ExpandAll hops surface their de-materialization mode.
        if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Graph(
            crate::query::executor::streaming::operators::spec::GraphSpec::ExpandAll {
                count_only,
                emit_raw_ids,
                ..
            },
        ) = &op_spec.spec
        {
            if *count_only {
                pairs.push(crate::query::planning::plan::explain::Pair::new(
                    "mode",
                    "count_only",
                ));
            } else if *emit_raw_ids {
                pairs.push(crate::query::planning::plan::explain::Pair::new(
                    "mode", "id_only",
                ));
            }
        }

        // Try to extract output column names from SourceSpec
        if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Source(src_spec) =
            &op_spec.spec
        {
            let col_names: Vec<String> = match src_spec {
                crate::query::executor::streaming::operators::spec::SourceSpec::ScanVertices { col_names, .. }
                | crate::query::executor::streaming::operators::spec::SourceSpec::StandaloneValues { col_names, .. }
                => col_names.clone(),
                crate::query::executor::streaming::operators::spec::SourceSpec::StorageScanVertices { col_names, .. } => col_names.clone(),
                crate::query::executor::streaming::operators::spec::SourceSpec::ScanEdges { col_names, .. } => col_names.clone(),
                crate::query::executor::streaming::operators::spec::SourceSpec::StorageScanEdges { col_names, .. } => col_names.clone(),
                _ => vec![],
            };
            if !col_names.is_empty() {
                pnd.output_var = col_names.join(", ");
            }
            // Surface the projected property list for storage scans so the
            // slot-coverage optimization (P1: EnrichScanSlotsWithFilterProps)
            // is observable in EXPLAIN output.
            let projected: Vec<String> = match src_spec {
                crate::query::executor::streaming::operators::spec::SourceSpec::StorageScanVertices { projected_properties, .. }
                | crate::query::executor::streaming::operators::spec::SourceSpec::StorageScanEdges { projected_properties, .. }
                => projected_properties.clone(),
                _ => vec![],
            };
            if !projected.is_empty() {
                pairs.push(crate::query::planning::plan::explain::Pair::new(
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
