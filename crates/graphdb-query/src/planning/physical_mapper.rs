//! Full logical-to-physical plan mapping.
//!
//! Converts a logical plan tree into a physical plan tree in one pass,
//! honoring the cost-based index hints recorded on logical scan nodes and
//! preserving factorization operators. The optimizer engine merges the
//! mapped tree with the cost-based physical tree so physical choices made
//! directly on the physical root (index scans with limits, TopN) survive.

use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::logical_nodes::access::{IndexHint, LogicalScanVerticesNode};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;

/// Full logical-to-physical mapper.
///
/// The entry points are [`PhysicalMapper::map`] (logical tree to physical
/// tree) and [`PhysicalMapper::merge_physical_hints`] (overlay of the
/// cost-based physical choices onto a mapped tree).
pub struct PhysicalMapper;

impl PhysicalMapper {
    /// Convert a logical plan tree into a physical plan tree.
    ///
    /// The structure mirrors the shared logical-to-physical converter;
    /// scan nodes carrying an index hint become index scans instead.
    pub fn map(logical: LogicalNodeEnum) -> PlanNodeEnum {
        let physical =
            crate::planning::physical_planner::convert_logical_to_physical(logical.clone());
        apply_hints(&logical, physical)
    }

    /// Report whether a logical tree needs the full mapping path.
    ///
    /// Trees without factorization operators or index hints map to the
    /// same structure the physical root already has, so the engine keeps
    /// the physical root untouched in that case.
    pub(crate) fn needs_physical_mapping(logical: &LogicalNodeEnum) -> bool {
        if matches!(logical, LogicalNodeEnum::Flatten(_)) {
            return true;
        }
        if let LogicalNodeEnum::ScanVertices(scan) = logical {
            if scan.index_hint.is_some() {
                return true;
            }
        }
        if let LogicalNodeEnum::ScanEdges(scan) = logical {
            if scan.index_hint.is_some() {
                return true;
            }
        }
        if let LogicalNodeEnum::GetNeighbors(node) = logical {
            if node.index_hint.is_some() {
                return true;
            }
        }
        logical_children(logical)
            .iter()
            .any(|child| Self::needs_physical_mapping(child))
    }

    /// Merge a fully mapped tree with the cost-based physical tree.
    ///
    /// The mapped tree supplies structure (notably factorization operator
    /// positions); the physical tree supplies cost-based access choices.
    /// Returns the merged tree plus notes for every position where the
    /// trees diverged and the physical choice won, so no fallback is
    /// silent.
    pub fn merge_physical_hints(
        mapped: PlanNodeEnum,
        physical: PlanNodeEnum,
    ) -> (PlanNodeEnum, Vec<String>) {
        let mut notes = Vec::new();
        let merged = merge_inner(mapped, physical, &mut notes);
        (merged, notes)
    }

    /// Collect factorization operator positions for plan diagnostics.
    pub(crate) fn collect_flatten_positions(node: &LogicalNodeEnum, out: &mut Vec<u32>) {
        if let LogicalNodeEnum::Flatten(flatten) = node {
            out.push(flatten.group_pos);
        }
        for child in logical_children(node) {
            Self::collect_flatten_positions(child, out);
        }
    }
}

/// Overlay index hints onto a converted physical tree.
///
/// The shared converter preserves tree shape one-to-one, so the logical
/// and physical trees can be walked in parallel: every logical scan that
/// carries an index hint replaces its physical full-scan counterpart.
fn apply_hints(logical: &LogicalNodeEnum, physical: PlanNodeEnum) -> PlanNodeEnum {
    if let LogicalNodeEnum::ScanVertices(scan) = logical {
        if let (Some(hint), PlanNodeEnum::ScanVertices(_)) = (&scan.index_hint, &physical) {
            return index_scan_from_hint(scan, hint);
        }
        return physical;
    }
    let logical_children = logical_children(logical);
    if logical_children.is_empty() {
        return physical;
    }
    let physical_children = physical.children();
    if logical_children.len() != physical_children.len() {
        return physical;
    }
    let mut new_children = Vec::with_capacity(logical_children.len());
    for (logical_child, physical_child) in logical_children.iter().zip(physical_children.iter()) {
        new_children.push(apply_hints(logical_child, (*physical_child).clone()));
    }
    rebuild_physical_with_new_children(&physical, new_children).unwrap_or(physical)
}

/// Build an index scan from a hinted logical scan.
///
/// Column bindings, the residual filter, the projection list and the
/// limit travel with the node; scan limits stay with the cost-based
/// physical choice and win during the merge when present.
fn index_scan_from_hint(scan: &LogicalScanVerticesNode, hint: &IndexHint) -> PlanNodeEnum {
    let mut node = IndexScanNode::new_with_str(
        scan.space_id,
        hint.tag_id,
        hint.index_id,
        &hint.index_name,
        &hint.schema_name,
        &hint.scan_type,
    );
    if let Some(expression) = &scan.expression {
        node.set_filter(expression.clone());
    }
    node.set_return_columns(scan.projected_properties.clone());
    if let Some(limit) = scan.limit {
        node.set_limit(limit);
    }
    if let Some(output_var) = &scan.output_var {
        node.set_output_var(output_var.clone());
    }
    node.set_col_names(scan.col_names.clone());
    node.set_column_types(scan.column_types.clone());
    PlanNodeEnum::IndexScan(node)
}

/// Merge one mapped/physical node pair; collects divergence notes.
fn merge_inner(
    mapped: PlanNodeEnum,
    physical: PlanNodeEnum,
    notes: &mut Vec<String>,
) -> PlanNodeEnum {
    // Factorization operators live only on the mapped side and are always
    // preserved; the merge continues below them against the same physical
    // node.
    if let PlanNodeEnum::Flatten(flatten) = mapped {
        let child = flatten.input().clone();
        let merged_child = merge_inner(child, physical, notes);
        let mut rebuilt = flatten.clone();
        rebuilt.set_input(merged_child);
        return PlanNodeEnum::Flatten(rebuilt);
    }
    // A cost-based index scan (with limits) always wins over a mapped
    // full scan or a limit-less mapped index scan.
    if let PlanNodeEnum::IndexScan(_) = &physical {
        if matches!(
            mapped,
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::IndexScan(_)
        ) {
            return physical;
        }
    }
    // A mapped index scan wins over a physical full scan: the logical
    // decision fired where the physical rewrite did not.
    if let PlanNodeEnum::IndexScan(_) = &mapped {
        if matches!(physical, PlanNodeEnum::ScanVertices(_)) {
            return mapped;
        }
    }
    // A wired TopN wins over the Sort+Limit shape it was built from.
    if let PlanNodeEnum::TopN(_) = &physical {
        if matches!(
            mapped,
            PlanNodeEnum::TopN(_) | PlanNodeEnum::Sort(_) | PlanNodeEnum::Limit(_)
        ) {
            return physical;
        }
    }
    if std::mem::discriminant(&mapped) == std::mem::discriminant(&physical) {
        let mapped_children = mapped.children();
        let physical_children = physical.children();
        if mapped_children.len() == physical_children.len() {
            let mut new_children = Vec::with_capacity(mapped_children.len());
            for (mapped_child, physical_child) in
                mapped_children.iter().zip(physical_children.iter())
            {
                new_children.push(merge_inner(
                    (*mapped_child).clone(),
                    (*physical_child).clone(),
                    notes,
                ));
            }
            match rebuild_physical_with_new_children(&physical, new_children) {
                Ok(rebuilt) => return rebuilt,
                Err(message) => {
                    notes.push(format!(
                        "PhysicalMapping: rebuild failed for {} ({message}); kept physical subtree",
                        physical.type_name()
                    ));
                    return physical;
                }
            }
        }
        notes.push(format!(
            "PhysicalMapping: child count diverged (mapped {} vs physical {}); kept physical subtree",
            mapped.type_name(),
            physical.type_name()
        ));
        return physical;
    }
    notes.push(format!(
        "PhysicalMapping: structure diverged (mapped {} vs physical {}); kept physical subtree",
        mapped.type_name(),
        physical.type_name()
    ));
    physical
}

pub(crate) fn logical_children(
    node: &crate::planning::plan::logical::LogicalNodeEnum,
) -> Vec<&crate::planning::plan::logical::LogicalNodeEnum> {
    use crate::planning::plan::logical::LogicalNodeEnum;
    match node {
        LogicalNodeEnum::Flatten(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Project(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Filter(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Sort(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Limit(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::TopN(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Sample(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Dedup(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Aggregate(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Window(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Traverse(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Assign(n) => {
            let mut v = Vec::new();
            if let Some(c) = n.input.as_deref() {
                v.push(c);
            }
            for d in &n.deps {
                v.push(d);
            }
            v
        }
        LogicalNodeEnum::Remove(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::DataCollect(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Materialize(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::RollUpApply(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Unwind(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
        LogicalNodeEnum::Select(n) => {
            let mut v = Vec::new();
            if let Some(b) = n.if_branch() {
                v.push(b);
            }
            if let Some(b) = n.else_branch() {
                v.push(b);
            }
            v
        }
        LogicalNodeEnum::Loop(n) => n.body().map(|b| vec![b]).unwrap_or_default(),
        LogicalNodeEnum::InnerJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::LeftJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::RightJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::CrossJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::FullOuterJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::SemiJoin(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::PatternApply(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::CorrelatedApply(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::Apply(n) => vec![n.left_input(), n.right_input()],
        LogicalNodeEnum::BiExpand(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::BiTraverse(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::MultiShortestPath(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::BFSShortest(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::AllPaths(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::ShortestPath(n) => vec![&n.left, &n.right],
        LogicalNodeEnum::Expand(n) => n.deps.iter().collect(),
        LogicalNodeEnum::ExpandAll(n) => n.deps.iter().collect(),
        LogicalNodeEnum::AppendVertices(n) => n.deps.iter().collect(),
        LogicalNodeEnum::GetVertices(n) => n.deps.iter().collect(),
        LogicalNodeEnum::GetNeighbors(n) => n.deps.iter().collect(),
        LogicalNodeEnum::Union(n) => n.deps.iter().collect(),
        LogicalNodeEnum::Minus(n) => n.deps.iter().collect(),
        LogicalNodeEnum::Intersect(n) => n.deps.iter().collect(),
        LogicalNodeEnum::WcoIntersect(n) => n.deps.iter().collect(),
        LogicalNodeEnum::Start(_)
        | LogicalNodeEnum::ScanVertices(_)
        | LogicalNodeEnum::ScanEdges(_)
        | LogicalNodeEnum::GetEdges(_)
        | LogicalNodeEnum::Argument(_)
        | LogicalNodeEnum::PassThrough(_)
        | LogicalNodeEnum::BeginTransaction(_)
        | LogicalNodeEnum::Commit(_)
        | LogicalNodeEnum::Rollback(_)
        | LogicalNodeEnum::FulltextSearch(_)
        | LogicalNodeEnum::FulltextLookup(_)
        | LogicalNodeEnum::MatchFulltext(_) => vec![],
        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorSearch(_)
        | LogicalNodeEnum::VectorLookup(_)
        | LogicalNodeEnum::VectorMatch(_) => vec![],
    }
}

pub(crate) fn rebuild_physical_with_new_children(
    physical: &crate::planning::plan::PlanNodeEnum,
    new_children: Vec<crate::planning::plan::PlanNodeEnum>,
) -> Result<crate::planning::plan::PlanNodeEnum, String> {
    use crate::planning::plan::core::nodes::base::plan_node_traits::{
        BinaryInputNode, MultipleInputNode, SingleInputNode,
    };
    use crate::planning::plan::PlanNodeEnum;
    match physical {
        PlanNodeEnum::Project(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Project")?,
            );
            Ok(PlanNodeEnum::Project(cloned))
        }
        PlanNodeEnum::Filter(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Filter")?,
            );
            Ok(PlanNodeEnum::Filter(cloned))
        }
        PlanNodeEnum::Sort(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Sort")?,
            );
            Ok(PlanNodeEnum::Sort(cloned))
        }
        PlanNodeEnum::Limit(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Limit")?,
            );
            Ok(PlanNodeEnum::Limit(cloned))
        }
        PlanNodeEnum::TopN(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for TopN")?,
            );
            Ok(PlanNodeEnum::TopN(cloned))
        }
        PlanNodeEnum::Sample(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Sample")?,
            );
            Ok(PlanNodeEnum::Sample(cloned))
        }
        PlanNodeEnum::Dedup(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Dedup")?,
            );
            Ok(PlanNodeEnum::Dedup(cloned))
        }
        PlanNodeEnum::Aggregate(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Aggregate")?,
            );
            Ok(PlanNodeEnum::Aggregate(cloned))
        }
        PlanNodeEnum::Window(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Window")?,
            );
            Ok(PlanNodeEnum::Window(cloned))
        }
        PlanNodeEnum::Traverse(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Traverse")?,
            );
            Ok(PlanNodeEnum::Traverse(cloned))
        }
        PlanNodeEnum::Unwind(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Unwind")?,
            );
            Ok(PlanNodeEnum::Unwind(cloned))
        }
        PlanNodeEnum::Assign(n) => {
            let mut cloned = n.clone();
            if new_children.is_empty() {
                return Err("missing children for Assign".to_string());
            }
            cloned.set_input(new_children[0].clone());
            if new_children.len() > 1 {
                let mut deps = cloned.dependencies().to_vec();
                for (i, c) in new_children.iter().skip(1).enumerate() {
                    if i < deps.len() {
                        deps[i] = c.clone();
                    }
                }
                cloned.set_dependencies(deps);
            }
            Ok(PlanNodeEnum::Assign(cloned))
        }
        PlanNodeEnum::DataCollect(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for DataCollect")?,
            );
            Ok(PlanNodeEnum::DataCollect(cloned))
        }
        PlanNodeEnum::Remove(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Remove")?,
            );
            Ok(PlanNodeEnum::Remove(cloned))
        }
        PlanNodeEnum::Materialize(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for Materialize")?,
            );
            Ok(PlanNodeEnum::Materialize(cloned))
        }
        PlanNodeEnum::RollUpApply(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for RollUpApply")?,
            );
            Ok(PlanNodeEnum::RollUpApply(cloned))
        }
        PlanNodeEnum::PatternApply(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for PatternApply")?,
            );
            Ok(PlanNodeEnum::PatternApply(cloned))
        }
        PlanNodeEnum::CorrelatedApply(n) => {
            let mut cloned = n.clone();
            cloned.set_input(
                new_children
                    .into_iter()
                    .next()
                    .ok_or("missing child for CorrelatedApply")?,
            );
            Ok(PlanNodeEnum::CorrelatedApply(cloned))
        }
        PlanNodeEnum::InnerJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("InnerJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::InnerJoin(cloned))
        }
        PlanNodeEnum::LeftJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("LeftJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::LeftJoin(cloned))
        }
        PlanNodeEnum::RightJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("RightJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::RightJoin(cloned))
        }
        PlanNodeEnum::CrossJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("CrossJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::CrossJoin(cloned))
        }
        PlanNodeEnum::FullOuterJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("FullOuterJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::FullOuterJoin(cloned))
        }
        PlanNodeEnum::SemiJoin(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("SemiJoin requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::SemiJoin(cloned))
        }
        PlanNodeEnum::Apply(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("Apply requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::Apply(cloned))
        }
        PlanNodeEnum::BiExpand(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("BiExpand requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::BiExpand(cloned))
        }
        PlanNodeEnum::BiTraverse(n) => {
            let mut cloned = n.clone();
            if new_children.len() != 2 {
                return Err("BiTraverse requires 2 children".to_string());
            }
            cloned.set_left_input(new_children[0].clone());
            cloned.set_right_input(new_children[1].clone());
            Ok(PlanNodeEnum::BiTraverse(cloned))
        }
        PlanNodeEnum::Expand(n) => {
            let mut cloned = n.clone();
            *cloned.inputs_mut() = new_children;
            Ok(PlanNodeEnum::Expand(cloned))
        }
        PlanNodeEnum::ExpandAll(n) => {
            let mut cloned = n.clone();
            *cloned.inputs_mut() = new_children;
            Ok(PlanNodeEnum::ExpandAll(cloned))
        }
        PlanNodeEnum::AppendVertices(n) => {
            let mut cloned = n.clone();
            *cloned.inputs_mut() = new_children;
            Ok(PlanNodeEnum::AppendVertices(cloned))
        }
        PlanNodeEnum::Union(n) => {
            let mut cloned = n.clone();
            *cloned.dependencies_mut() = new_children.clone();
            if let Some(first) = new_children.first() {
                *cloned.input_mut() = first.clone();
            }
            Ok(PlanNodeEnum::Union(cloned))
        }
        PlanNodeEnum::Minus(n) => {
            let mut cloned = n.clone();
            *cloned.dependencies_mut() = new_children.clone();
            if let Some(first) = new_children.first() {
                *cloned.input_mut() = first.clone();
            }
            Ok(PlanNodeEnum::Minus(cloned))
        }
        PlanNodeEnum::Intersect(n) => {
            let mut cloned = n.clone();
            *cloned.dependencies_mut() = new_children.clone();
            if let Some(first) = new_children.first() {
                *cloned.input_mut() = first.clone();
            }
            Ok(PlanNodeEnum::Intersect(cloned))
        }
        PlanNodeEnum::WcoIntersect(n) => {
            let mut cloned = n.clone();
            *cloned.dependencies_mut() = new_children.clone();
            if let Some(first) = new_children.first() {
                *cloned.input_mut() = first.clone();
            }
            Ok(PlanNodeEnum::WcoIntersect(cloned))
        }
        PlanNodeEnum::Select(n) => {
            let mut cloned = n.clone();
            let orig_has_if = n.if_branch().is_some();
            let orig_has_else = n.else_branch().is_some();
            let mut idx = 0;
            if orig_has_if && idx < new_children.len() {
                cloned.set_if_branch(new_children[idx].clone());
                idx += 1;
            }
            if orig_has_else && idx < new_children.len() {
                cloned.set_else_branch(new_children[idx].clone());
            }
            Ok(PlanNodeEnum::Select(cloned))
        }
        PlanNodeEnum::Loop(n) => {
            let mut cloned = n.clone();
            if let Some(new_body) = new_children.into_iter().next() {
                cloned.set_body(new_body);
            }
            Ok(PlanNodeEnum::Loop(cloned))
        }
        _ => Ok(physical.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
    use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode;
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;
    use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::Expression;
    use graphdb_core::types::expr::{ContextualExpression, ExpressionMeta};
    use graphdb_core::types::operators::BinaryOperator;
    use graphdb_core::value::Value;
    use std::sync::Arc;

    fn hinted_scan() -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: 1,
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some("person".to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            index_hint: Some(IndexHint::new(
                "idx_name".to_string(),
                "person".to_string(),
                7,
                9,
                "RANGE".to_string(),
            )),
            estimated_cardinality: Some(42),
            output_var: Some("n".to_string()),
            col_names: vec!["n".to_string()],
            column_types: vec![],
        })
    }

    fn filter_over(logical_input: LogicalNodeEnum) -> LogicalNodeEnum {
        let context = Arc::new(ExpressionAnalysisContext::new());
        let expression = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "name".to_string(),
            }),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::String("alice".into()))),
        };
        let id = context.register_expression(ExpressionMeta::new(expression));
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: 2,
            input: Some(Box::new(logical_input.clone())),
            deps: vec![logical_input],
            condition: ContextualExpression::new(id, context),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn map_hinted_scan_becomes_index_scan() {
        let mapped = PhysicalMapper::map(filter_over(hinted_scan()));
        let filter = match &mapped {
            PlanNodeEnum::Filter(f) => f,
            other => panic!("expected physical filter, got {}", other.type_name()),
        };
        match filter.input() {
            PlanNodeEnum::IndexScan(scan) => {
                assert_eq!(scan.index_name(), "idx_name");
                assert_eq!(scan.tag_id(), 7);
                assert_eq!(scan.index_id(), 9);
            }
            other => panic!("expected index scan, got {}", other.type_name()),
        }
    }

    #[test]
    fn merge_preserves_flatten_and_physical_index_scan() {
        let mapped = PhysicalMapper::map(LogicalNodeEnum::Flatten(LogicalFlattenNode::new(
            0,
            hinted_scan(),
        )));
        let mut physical_scan = IndexScanNode::new(
            1,
            7,
            9,
            "idx_name".to_string(),
            "person".to_string(),
            crate::planning::plan::core::nodes::access::index_scan::ScanType::Range,
        );
        physical_scan.set_col_names(vec!["n".to_string()]);
        let physical = PlanNodeEnum::IndexScan(physical_scan);
        let (merged, notes) = PhysicalMapper::merge_physical_hints(mapped, physical);
        assert!(notes.is_empty());
        let flatten = match &merged {
            PlanNodeEnum::Flatten(f) => f,
            other => panic!("expected flatten, got {}", other.type_name()),
        };
        assert!(matches!(flatten.input(), PlanNodeEnum::IndexScan(_)));
    }

    #[test]
    fn merge_divergence_keeps_physical_with_note() {
        let mapped = PhysicalMapper::map(hinted_scan());
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_col_names(vec!["n".to_string()]);
        let context = Arc::new(ExpressionAnalysisContext::new());
        let id =
            context.register_expression(ExpressionMeta::new(Expression::Literal(Value::Int(1))));
        let physical = PlanNodeEnum::Filter(
            FilterNode::new(
                PlanNodeEnum::ScanVertices(scan),
                ContextualExpression::new(id, context),
            )
            .expect("filter"),
        );
        let (merged, notes) = PhysicalMapper::merge_physical_hints(mapped, physical);
        assert!(matches!(merged, PlanNodeEnum::Filter(_)));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("diverged"));
    }

    #[test]
    fn unmapped_tree_needs_no_mapping() {
        let mut scan = hinted_scan();
        if let LogicalNodeEnum::ScanVertices(s) = &mut scan {
            s.index_hint = None;
            s.estimated_cardinality = None;
        }
        assert!(!PhysicalMapper::needs_physical_mapping(&scan));
        assert!(PhysicalMapper::needs_physical_mapping(&hinted_scan()));
    }

    #[test]
    fn flatten_rebuild_roundtrip() {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_col_names(vec!["n".to_string()]);
        let child = PlanNodeEnum::ScanVertices(scan);
        let flatten = FlattenNode::new(child, 0).expect("flatten");
        let rebuilt = rebuild_physical_with_new_children(
            &PlanNodeEnum::Flatten(flatten),
            vec![PlanNodeEnum::ScanVertices({
                let mut s = ScanVerticesNode::new(1, "test");
                s.set_col_names(vec!["n".to_string()]);
                s
            })],
        )
        .expect("rebuild");
        assert!(matches!(rebuilt, PlanNodeEnum::Flatten(_)));
    }
}
