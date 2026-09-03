//! Normalized cardinality shape keys for cost-feedback corrections.

use super::{ApplySpec, BlockingSpec, GraphSpec, JoinSpec, SetSpec, SourceSpec, UnarySpec};
use crate::executor::streaming::plan::types::OperatorKindSpec;

/// Normalized shape key for an operator's output cardinality.
///
/// Returns `"{space}:{Type}:{discriminator}"` for operators whose output row
/// count is estimated independently (sources, graph traversals, joins,
/// applies, aggregates).  Filter operators return `None`: they are corrected
/// per predicate via `condition_key` in the selectivity feedback loop.
///
/// The string format must stay in sync with the plan-side key generator in
/// `optimizer/cost_based/row_estimates.rs` (`cardinality_shape_key`), so
/// corrections recorded against physical operators are applied to the same
/// shapes during cost-based estimation.
pub fn operator_cardinality_shape_key(
    space: Option<&str>,
    spec: &OperatorKindSpec,
) -> Option<String> {
    let prefix = space.unwrap_or("").to_string();
    let key = |kind: &str, discriminator: Option<&str>| {
        let mut key = format!("{prefix}:{kind}");
        if let Some(discriminator) = discriminator {
            if !discriminator.is_empty() {
                key.push(':');
                key.push_str(discriminator);
            }
        }
        Some(key)
    };
    let join_types = |kind: &str| key(kind, None);
    match spec {
        OperatorKindSpec::Source(source) => match source {
            SourceSpec::Start | SourceSpec::Argument { .. } => None,
            SourceSpec::ScanVertices { col_names, .. } => {
                key("ScanVertices", col_names.first().map(String::as_str))
            }
            SourceSpec::StandaloneValues { .. } => None,
            SourceSpec::StorageScanVertices { tag, .. } => key("ScanVertices", tag.as_deref()),
            SourceSpec::ScanEdges { col_names, .. } => {
                key("ScanEdges", col_names.first().map(String::as_str))
            }
            SourceSpec::StorageScanEdges { edge_type, .. } => {
                key("ScanEdges", edge_type.as_deref())
            }
            SourceSpec::GetVertices { .. } => key("GetVertices", None),
            SourceSpec::GetEdges { edge_type, .. } => key("GetEdges", edge_type.as_deref()),
            SourceSpec::GetNeighbors { direction, .. } => key("GetNeighbors", Some(direction)),
            SourceSpec::IndexScan { index_name, .. } => key("IndexScan", Some(index_name)),
            SourceSpec::GetProp { .. } => None,
        },
        OperatorKindSpec::Unary(UnarySpec::Filter { .. }) => None,
        OperatorKindSpec::Unary(UnarySpec::AppendVertices { entity_var, .. }) => {
            key("AppendVertices", Some(entity_var))
        }
        OperatorKindSpec::Unary(_) => None,
        OperatorKindSpec::Blocking(
            BlockingSpec::Aggregate { .. }
            | BlockingSpec::PartialAggregate { .. }
            | BlockingSpec::FinalAggregate { .. },
        ) => key("Aggregate", None),
        OperatorKindSpec::Blocking(_) => None,
        OperatorKindSpec::Join(spec) => match spec {
            JoinSpec::InnerJoin { .. } => join_types("InnerJoin"),
            JoinSpec::LeftJoin { .. } => join_types("LeftJoin"),
            JoinSpec::RightJoin { .. } => join_types("RightJoin"),
            JoinSpec::FullOuterJoin { .. } => join_types("FullOuterJoin"),
            JoinSpec::CrossJoin => join_types("CrossJoin"),
            JoinSpec::SemiJoin { .. } => join_types("SemiJoin"),
            JoinSpec::HashJoin { .. } => join_types("HashJoin"),
            JoinSpec::HashLeftJoin { .. } => join_types("HashLeftJoin"),
            JoinSpec::NestedLoopJoin { .. } => join_types("NestedLoopJoin"),
        },
        OperatorKindSpec::Graph(spec) => match spec {
            GraphSpec::Expand { edge_types, .. } => key("Expand", Some(&edge_types.join(","))),
            GraphSpec::ExpandAll { edge_types, .. } => {
                key("ExpandAll", Some(&edge_types.join(",")))
            }
            GraphSpec::Traverse { edge_types, .. } => key("Traverse", Some(&edge_types.join(","))),
            GraphSpec::BiExpand { edge_types, .. } => key("BiExpand", Some(&edge_types.join(","))),
            GraphSpec::BiTraverse { edge_types, .. } => {
                key("BiTraverse", Some(&edge_types.join(",")))
            }
        },
        OperatorKindSpec::Apply(spec) => match spec {
            ApplySpec::Apply { .. } => key("Apply", None),
            ApplySpec::PatternApply { .. } => key("PatternApply", None),
            ApplySpec::CorrelatedApply { .. } => key("CorrelatedApply", None),
            ApplySpec::RollUpApply { .. } => key("RollUpApply", None),
        },
        OperatorKindSpec::Set(spec) => match spec {
            SetSpec::Union | SetSpec::UnionAll => key("Union", None),
            SetSpec::Intersect => key("Intersect", None),
            SetSpec::Except | SetSpec::Minus => key("Minus", None),
        },
        _ => None,
    }
}
