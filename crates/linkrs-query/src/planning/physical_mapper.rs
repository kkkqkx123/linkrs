//! Full logical-to-physical plan mapping.
//!
//! Converts a logical plan tree into a physical plan tree in one pass,
//! honoring the cost-based index hints recorded on logical scan nodes and
//! preserving factorization operators. The optimizer engine merges the
//! mapped tree with the cost-based physical tree so physical choices made
//! directly on the physical root (index scans with limits, TopN) survive.

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;

/// Failure to overlay a mapped logical tree onto the cost-based physical tree.
///
/// Carries both operator names plus the child-index path from the root so the
/// caller can locate the divergence without guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhysicalMappingError {
    /// Mapped and physical nodes share a discriminant but differ in arity.
    ChildCountDiverged {
        path: String,
        mapped: String,
        physical: String,
    },
    /// Same-discriminant children merged but the parent could not be rebuilt.
    RebuildFailed {
        path: String,
        node: String,
        reason: String,
    },
    /// Mapped and physical nodes differ in operator kind.
    StructureDiverged {
        path: String,
        mapped: String,
        physical: String,
    },
}

impl std::fmt::Display for PhysicalMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChildCountDiverged {
                path,
                mapped,
                physical,
            } => write!(
                f,
                "physical mapping child count diverged at {path} (mapped {mapped} vs physical {physical})"
            ),
            Self::RebuildFailed { path, node, reason } => write!(
                f,
                "physical mapping rebuild failed at {path} for {node}: {reason}"
            ),
            Self::StructureDiverged {
                path,
                mapped,
                physical,
            } => write!(
                f,
                "physical mapping structure diverged at {path} (mapped {mapped} vs physical {physical})"
            ),
        }
    }
}

impl std::error::Error for PhysicalMappingError {}

/// Full logical-to-physical mapper.
///
/// The entry points are [`PhysicalMapper::map`] (logical tree to physical
/// tree) and [`PhysicalMapper::merge_physical_hints`] (overlay of the
/// cost-based physical choices onto a mapped tree).
///
/// Statement planners use `map` as the single construction entry so index
/// hints and factorization positions are applied uniformly. The converter
/// in `physical_planner` is the shared recursive implementation backing
/// this entry, not a parallel statement-level path.
pub struct PhysicalMapper;

impl PhysicalMapper {
    /// Convert a logical plan tree into a physical plan tree.
    ///
    /// Index hints travel with the conversion itself: the shared converter
    /// emits an index scan directly for hinted scans, so no post-pass overlay
    /// is needed.
    pub fn map(logical: LogicalNodeEnum) -> PlanNodeEnum {
        crate::planning::physical_planner::convert_logical_to_physical(logical)
    }

    /// Report whether a logical tree needs the full mapping path.
    ///
    /// Trees without factorization operators or index hints map to the
    /// same structure the physical root already has, so the engine keeps
    /// the physical root untouched in that case. Only scan variants that the
    /// converter can turn into index scans participate; neighbor expansion
    /// carries no index overlay and never triggers a remap.
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
        logical
            .children_ref()
            .iter()
            .any(|child| Self::needs_physical_mapping(child))
    }

    /// Merge a fully mapped tree with the cost-based physical tree.
    ///
    /// The mapped tree supplies structure (notably factorization operator
    /// positions); the physical tree supplies cost-based access choices.
    /// Intentional overlays (factorization preserved, cost-based index scan
    /// or wired TopN winning) succeed. Any other structural divergence is an
    /// explicit error carrying both operator names and the tree path.
    pub fn merge_physical_hints(
        mapped: PlanNodeEnum,
        physical: PlanNodeEnum,
    ) -> Result<PlanNodeEnum, PhysicalMappingError> {
        let mut mapped = mapped;
        merge_inner(&mut mapped, physical, &mut Vec::new(), "root")?;
        Ok(mapped)
    }

    /// Collect factorization operator positions for plan diagnostics.
    pub(crate) fn collect_flatten_positions(node: &LogicalNodeEnum, out: &mut Vec<u32>) {
        if let LogicalNodeEnum::Flatten(flatten) = node {
            out.push(flatten.group_pos);
        }
        for child in node.children_ref() {
            Self::collect_flatten_positions(child, out);
        }
    }
}

/// Merge one mapped/physical node pair.
///
/// The mapped tree is rewritten in place while the physical tree is consumed
/// by value, so each recursion level holds one pointer plus one owned node
/// instead of two owned trees. `path` tracks the child-index route from the
/// root for error reports.
fn merge_inner(
    mapped: &mut PlanNodeEnum,
    physical: PlanNodeEnum,
    child_path: &mut Vec<usize>,
    path: &str,
) -> Result<(), PhysicalMappingError> {
    // Factorization operators live only on the mapped side and are always
    // preserved; the merge continues below them against the same physical
    // node.
    if let PlanNodeEnum::Flatten(flatten) = mapped {
        let placeholder = PlanNodeEnum::Start(crate::planning::plan::core::nodes::StartNode::new());
        let mut child = std::mem::replace(flatten.input_mut(), placeholder);
        let result = merge_inner(&mut child, physical, child_path, path);
        flatten.set_input(child);
        return result;
    }
    // A cost-based index scan (with limits) always wins over a mapped
    // full scan or a limit-less mapped index scan.
    if let PlanNodeEnum::IndexScan(_) = &physical {
        if matches!(
            &*mapped,
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::IndexScan(_)
        ) {
            *mapped = physical;
            return Ok(());
        }
    }
    // A mapped index scan wins over a physical full scan: the logical
    // decision fired where the physical rewrite did not.
    if let PlanNodeEnum::IndexScan(_) = &*mapped {
        if matches!(
            &physical,
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_)
        ) {
            return Ok(());
        }
    }
    // A wired TopN wins over the Sort+Limit shape it was built from.
    if let PlanNodeEnum::TopN(_) = &physical {
        if matches!(
            &*mapped,
            PlanNodeEnum::TopN(_) | PlanNodeEnum::Sort(_) | PlanNodeEnum::Limit(_)
        ) {
            *mapped = physical;
            return Ok(());
        }
    }
    if std::mem::discriminant(&*mapped) == std::mem::discriminant(&physical) {
        use crate::optimizer::cost::child_accessor::ChildAccessor;

        if mapped.child_count() != physical.child_count() {
            return Err(PhysicalMappingError::ChildCountDiverged {
                path: path.to_string(),
                mapped: mapped.type_name().to_string(),
                physical: physical.type_name().to_string(),
            });
        }
        let mut physical = physical;
        let mut mapped_children = mapped.take_children();
        let physical_children = physical.take_children();
        let mut new_children = Vec::with_capacity(mapped_children.len());
        for (index, (mapped_child, physical_child)) in
            mapped_children.drain(..).zip(physical_children).enumerate()
        {
            let mut merged_child = mapped_child;
            child_path.push(index);
            let child_label = format!("{path}.{index}");
            merge_inner(&mut merged_child, physical_child, child_path, &child_label)?;
            child_path.pop();
            new_children.push(merged_child);
        }
        match physical.set_children(new_children) {
            Ok(()) => {
                *mapped = physical;
                return Ok(());
            }
            Err(message) => {
                return Err(PhysicalMappingError::RebuildFailed {
                    path: path.to_string(),
                    node: physical.type_name().to_string(),
                    reason: message,
                });
            }
        }
    }
    Err(PhysicalMappingError::StructureDiverged {
        path: path.to_string(),
        mapped: mapped.type_name().to_string(),
        physical: physical.type_name().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
    use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode;
    use crate::planning::plan::logical::logical_nodes::access::{
        IndexHint, LogicalScanVerticesNode,
    };
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;
    use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::Expression;
    use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::value::Value;
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
            input: Some(Box::new(logical_input)),
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
        let merged = PhysicalMapper::merge_physical_hints(mapped, physical)
            .expect("flatten plus index scan merges");
        let flatten = match &merged {
            PlanNodeEnum::Flatten(f) => f,
            other => panic!("expected flatten, got {}", other.type_name()),
        };
        assert!(matches!(flatten.input(), PlanNodeEnum::IndexScan(_)));
    }

    #[test]
    fn merge_divergence_reports_explicit_error() {
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
        let err = PhysicalMapper::merge_physical_hints(mapped, physical)
            .expect_err("structure divergence must fail explicitly");
        match err {
            PhysicalMappingError::StructureDiverged { .. } => {}
            other => panic!("expected structure divergence, got {other:?}"),
        }
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
