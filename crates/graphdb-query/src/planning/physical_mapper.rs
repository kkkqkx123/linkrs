//! Full logical-to-physical plan mapping.
//!
//! Converts a logical plan tree into a physical plan tree in one pass,
//! honoring the cost-based index hints recorded on logical scan nodes and
//! preserving factorization operators. The optimizer engine merges the
//! mapped tree with the cost-based physical tree so physical choices made
//! directly on the physical root (index scans with limits, TopN) survive.

use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::logical_nodes::access::{
    IndexHint, LogicalScanEdgesNode, LogicalScanVerticesNode,
};
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
        // Collect index hints by reference first so the converter below can
        // consume the logical tree by value: only hinted leaf scans are
        // cloned instead of the whole tree.
        let hints = collect_index_hints(&logical);
        let physical = crate::planning::physical_planner::convert_logical_to_physical(logical);
        apply_collected_hints(physical, &hints)
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

/// A hinted leaf scan collected from the logical tree with its position.
///
/// Only the scan node itself is cloned (it carries no subtree); the path
/// records child indices from the root following [`logical_children`] order,
/// which the shared converter preserves one-to-one.
struct IndexHintTarget {
    path: Vec<usize>,
    scan: HintedScan,
}

/// A logical scan node carrying an index hint.
enum HintedScan {
    Vertices(LogicalScanVerticesNode),
    Edges(LogicalScanEdgesNode),
}

/// Walk the logical tree by reference and collect every hinted scan.
fn collect_index_hints(logical: &LogicalNodeEnum) -> Vec<IndexHintTarget> {
    fn walk(node: &LogicalNodeEnum, path: &mut Vec<usize>, out: &mut Vec<IndexHintTarget>) {
        match node {
            LogicalNodeEnum::ScanVertices(scan) if scan.index_hint.is_some() => {
                out.push(IndexHintTarget {
                    path: path.clone(),
                    scan: HintedScan::Vertices(scan.clone()),
                });
            }
            LogicalNodeEnum::ScanEdges(scan) if scan.index_hint.is_some() => {
                out.push(IndexHintTarget {
                    path: path.clone(),
                    scan: HintedScan::Edges(scan.clone()),
                });
            }
            _ => {}
        }
        for (index, child) in logical_children(node).iter().enumerate() {
            path.push(index);
            walk(child, path, out);
            path.pop();
        }
    }

    let mut out = Vec::new();
    walk(logical, &mut Vec::new(), &mut out);
    out
}

/// Overlay collected index hints onto a converted physical tree.
///
/// Each hint descends by its recorded child-index path; hints whose path no
/// longer resolves (or whose target is not the matching scan) are skipped,
/// keeping the converted node unchanged.
fn apply_collected_hints(mut physical: PlanNodeEnum, hints: &[IndexHintTarget]) -> PlanNodeEnum {
    for hint in hints {
        let Some(target) = descend_to_path(&mut physical, &hint.path) else {
            continue;
        };
        let replacement = match (&hint.scan, &*target) {
            (HintedScan::Vertices(scan), PlanNodeEnum::ScanVertices(_)) => scan
                .index_hint
                .as_ref()
                .map(|hint| index_scan_from_hint(scan, hint)),
            (HintedScan::Edges(scan), PlanNodeEnum::ScanEdges(_)) => scan
                .index_hint
                .as_ref()
                .map(|hint| index_scan_from_edge_hint(scan, hint)),
            _ => None,
        };
        if let Some(node) = replacement {
            *target = node;
        }
    }
    physical
}

/// Descend a physical tree by child-index path, returning the target slot.
///
/// Returns `None` when any step does not resolve, mirroring the previous
/// parallel walk's shape-divergence fallback.
fn descend_to_path<'a>(
    mut node: &'a mut PlanNodeEnum,
    path: &[usize],
) -> Option<&'a mut PlanNodeEnum> {
    use crate::optimizer::cost::child_accessor::ChildAccessor;

    for &index in path {
        node = node.get_child_mut(index)?;
    }
    Some(node)
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

/// Build an index scan from a hinted logical edge scan.
///
/// Mirrors [`index_scan_from_hint`] for edge lookups: the edge type name
/// travels as the schema name and the tag id is 0 (edges have no tag id).
fn index_scan_from_edge_hint(scan: &LogicalScanEdgesNode, hint: &IndexHint) -> PlanNodeEnum {
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
    if let PlanNodeEnum::Flatten(mut flatten) = mapped {
        let placeholder = PlanNodeEnum::Start(crate::planning::plan::core::nodes::StartNode::new());
        let child = std::mem::replace(flatten.input_mut(), placeholder);
        let merged_child = merge_inner(child, physical, notes);
        flatten.set_input(merged_child);
        return PlanNodeEnum::Flatten(flatten);
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
        if matches!(
            physical,
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_)
        ) {
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
        use crate::optimizer::cost::child_accessor::ChildAccessor;

        // Compare child counts before detaching either side so the
        // divergence fallback below returns the untouched physical subtree.
        if mapped.child_count() != physical.child_count() {
            notes.push(format!(
                "PhysicalMapping: child count diverged (mapped {} vs physical {}); kept physical subtree",
                mapped.type_name(),
                physical.type_name()
            ));
            return physical;
        }
        let mut mapped = mapped;
        let mut physical = physical;
        let mapped_children = mapped.take_children();
        let physical_children = physical.take_children();
        let mut new_children = Vec::with_capacity(mapped_children.len());
        for (mapped_child, physical_child) in mapped_children
            .into_iter()
            .zip(physical_children.into_iter())
        {
            new_children.push(merge_inner(mapped_child, physical_child, notes));
        }
        match physical.set_children(new_children) {
            Ok(()) => return physical,
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
        LogicalNodeEnum::Skip(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
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
        LogicalNodeEnum::PipeDeleteVertices(n) => {
            n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
        }
        LogicalNodeEnum::PipeDeleteEdges(n) => {
            n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
        }
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
        | LogicalNodeEnum::InsertVertices(_)
        | LogicalNodeEnum::InsertEdges(_)
        | LogicalNodeEnum::Update(_)
        | LogicalNodeEnum::DeleteVertices(_)
        | LogicalNodeEnum::DeleteEdges(_)
        | LogicalNodeEnum::DeleteTags(_)
        | LogicalNodeEnum::DeleteIndex(_)
        | LogicalNodeEnum::CopyFrom(_)
        | LogicalNodeEnum::CopyTo(_)
        | LogicalNodeEnum::FulltextSearch(_)
        | LogicalNodeEnum::FulltextLookup(_)
        | LogicalNodeEnum::MatchFulltext(_) => vec![],
        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorSearch(_)
        | LogicalNodeEnum::VectorLookup(_)
        | LogicalNodeEnum::VectorMatch(_) => vec![],
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
    fn map_flatten_preserves_group_column_mapping() {
        let mut flatten = LogicalFlattenNode::new(2, hinted_scan());
        flatten.set_group_columns(vec!["b".to_string()]);
        flatten.set_expected_groups(3);
        let mapped = PhysicalMapper::map(LogicalNodeEnum::Flatten(flatten));
        match &mapped {
            PlanNodeEnum::Flatten(f) => {
                assert_eq!(f.group_pos(), 2);
                assert_eq!(f.group_columns(), &["b".to_string()]);
                assert_eq!(f.expected_groups(), Some(3));
            }
            other => panic!("expected physical flatten, got {}", other.type_name()),
        }
    }

    #[test]
    fn map_hinted_scan_under_flatten_becomes_index_scan() {
        let mapped = PhysicalMapper::map(LogicalNodeEnum::Flatten(LogicalFlattenNode::new(
            0,
            hinted_scan(),
        )));
        let flatten = match &mapped {
            PlanNodeEnum::Flatten(f) => f,
            other => panic!("expected physical flatten, got {}", other.type_name()),
        };
        match flatten.input() {
            PlanNodeEnum::IndexScan(scan) => {
                assert_eq!(scan.index_name(), "idx_name");
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
    fn flatten_take_set_roundtrip() {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_col_names(vec!["n".to_string()]);
        let child = PlanNodeEnum::ScanVertices(scan);
        let flatten = FlattenNode::new(child, 0).expect("flatten");
        let mut node = PlanNodeEnum::Flatten(flatten);
        let taken = node.take_children();
        assert_eq!(taken.len(), 1);
        node.set_children(vec![PlanNodeEnum::ScanVertices({
            let mut s = ScanVerticesNode::new(1, "test");
            s.set_col_names(vec!["n".to_string()]);
            s
        })])
        .expect("set");
        assert!(matches!(node, PlanNodeEnum::Flatten(_)));
    }
}
