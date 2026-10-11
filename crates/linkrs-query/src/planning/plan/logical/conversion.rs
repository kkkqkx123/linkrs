//! Conversion from `PlanNodeEnum` to `LogicalNodeEnum`.
//!
//! This module implements the physical-to-logical compatibility bridge used
//! only when a planner did not emit a native logical tree. Factorized
//! planners attach the logical tree directly, so this reverse pass runs
//! solely for statements that stay physical-only by design.
//! - Physical operators (IndexScan, InnerJoin, LeftJoin)
//!   are mapped to their logical equivalents.
//! - All other operators are recursively converted.

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

/// Errors that can occur during physical-to-logical conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// The plan contains a node type not yet supported in conversion.
    NotYetImplemented(String),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotYetImplemented(name) => {
                write!(f, "conversion not yet implemented for: {}", name)
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// Recursively convert a `PlanNodeEnum` tree to a `LogicalNodeEnum` tree.
///
/// # Physical-to-Logical Mapping
/// - `IndexScan` → `ScanVertices`
/// - `InnerJoin` → `InnerJoin`
/// - `LeftJoin` → `LeftJoin`
/// - All other logical operators are preserved with recursive child conversion.
///
/// # Limitations
/// Statements that remain physical-only by design return `NotYetImplemented`
/// and keep flat execution without factorization.
/// - `IndexScan` → `ScanVertices`
/// - `InnerJoin` → `InnerJoin`
/// - `LeftJoin` → `LeftJoin`
/// - All other logical operators are preserved with recursive child conversion.
///
/// # Limitations
/// Statements that remain physical-only by design return `NotYetImplemented`
/// and keep flat execution without factorization.
pub fn convert_plan(node: &PlanNodeEnum) -> Result<LogicalNodeEnum, ConversionError> {
    match node {
        // === Access nodes ===
        PlanNodeEnum::Start(n) => access::convert_start(n),
        PlanNodeEnum::ScanVertices(n) => access::convert_scan_vertices(n),
        PlanNodeEnum::ScanEdges(n) => access::convert_scan_edges(n),
        PlanNodeEnum::GetVertices(n) => access::convert_get_vertices(n),
        PlanNodeEnum::GetNeighbors(n) => access::convert_get_neighbors(n),
        PlanNodeEnum::IndexScan(n) => access::convert_index_scan(n),
        // === Operation nodes ===
        PlanNodeEnum::Project(n) => operation::convert_project(n),
        PlanNodeEnum::Filter(n) => operation::convert_filter(n),
        PlanNodeEnum::Sort(n) => operation::convert_sort(n),
        PlanNodeEnum::Limit(n) => operation::convert_limit(n),
        PlanNodeEnum::TopN(n) => operation::convert_top_n(n),
        PlanNodeEnum::Sample(n) => operation::convert_sample(n),
        PlanNodeEnum::Dedup(n) => operation::convert_dedup(n),
        PlanNodeEnum::Aggregate(n) => operation::convert_aggregate(n),
        PlanNodeEnum::Window(n) => operation::convert_window(n),
        // === Join nodes ===
        PlanNodeEnum::InnerJoin(n) => join::convert_inner_join(n),
        PlanNodeEnum::LeftJoin(n) => join::convert_left_join(n),
        PlanNodeEnum::RightJoin(n) => join::convert_right_join(n),
        PlanNodeEnum::CrossJoin(n) => join::convert_cross_join(n),
        PlanNodeEnum::FullOuterJoin(n) => join::convert_full_outer_join(n),
        PlanNodeEnum::SemiJoin(n) => join::convert_semi_join(n),
        PlanNodeEnum::Flatten(n) => join::convert_flatten(n),
        // === Traversal nodes ===
        PlanNodeEnum::Expand(n) => traversal::convert_expand(n),
        PlanNodeEnum::ExpandAll(n) => traversal::convert_expand_all(n),
        PlanNodeEnum::Traverse(n) => traversal::convert_traverse(n),
        PlanNodeEnum::AppendVertices(n) => traversal::convert_append_vertices(n),
        PlanNodeEnum::BiExpand(n) => traversal::convert_bi_expand(n),
        PlanNodeEnum::BiTraverse(n) => traversal::convert_bi_traverse(n),
        // === Control flow nodes ===
        PlanNodeEnum::Select(n) => control_flow::convert_select(n),
        PlanNodeEnum::Loop(n) => control_flow::convert_loop(n),
        PlanNodeEnum::Argument(n) => control_flow::convert_argument(n),
        PlanNodeEnum::PassThrough(n) => control_flow::convert_pass_through(n),
        // === Pipeline nodes ===
        PlanNodeEnum::Assign(n) => pipeline::convert_assign(n),
        PlanNodeEnum::Remove(n) => pipeline::convert_remove(n),
        PlanNodeEnum::DataCollect(n) => pipeline::convert_data_collect(n),
        PlanNodeEnum::Materialize(n) => pipeline::convert_materialize(n),
        // === Set operation nodes ===
        PlanNodeEnum::Union(n) => setop::convert_union(n),
        PlanNodeEnum::Minus(n) => setop::convert_minus(n),
        PlanNodeEnum::Intersect(n) => setop::convert_intersect(n),
        PlanNodeEnum::Unwind(n) => setop::convert_unwind(n),
        // === Apply nodes ===
        PlanNodeEnum::Apply(n) => apply::convert_apply(n),
        PlanNodeEnum::PatternApply(n) => apply::convert_pattern_apply(n),
        PlanNodeEnum::RollUpApply(n) => apply::convert_roll_up_apply(n),
        PlanNodeEnum::CorrelatedApply(n) => apply::convert_correlated_apply(n),
        PlanNodeEnum::WcoIntersect(n) => apply::convert_wco_intersect(n),
        // === Write nodes ===
        PlanNodeEnum::GetEdges(n) => write::convert_get_edges(n),
        PlanNodeEnum::InsertVertices(n) => write::convert_insert_vertices(n),
        PlanNodeEnum::InsertEdges(n) => write::convert_insert_edges(n),
        PlanNodeEnum::Update(n) => write::convert_update(n),
        PlanNodeEnum::DeleteVertices(n) => write::convert_delete_vertices(n),
        PlanNodeEnum::DeleteEdges(n) => write::convert_delete_edges(n),
        PlanNodeEnum::DeleteIndex(n) => write::convert_delete_index(n),
        PlanNodeEnum::PipeDeleteVertices(n) => write::convert_pipe_delete_vertices(n),
        PlanNodeEnum::PipeDeleteEdges(n) => write::convert_pipe_delete_edges(n),
        PlanNodeEnum::CopyFrom(n) => write::convert_copy_from(n),
        PlanNodeEnum::CopyTo(n) => write::convert_copy_to(n),
        // === Fallback for unsupported nodes ===
        // === Fallback for unsupported nodes ===
        // Intentionally not mapped: Algorithm nodes (MultiShortestPath/BFSShortest/AllPaths/ShortestPath)
        // and Fulltext/Vector search remain `NotYetImplemented`.
        // They are non-factorized operators not on the factorization / partitioning chain,
        // so reverse mapping is optional; `ensure_logical_plan` will record the failure in
        // `cbo_notes` and fall back to flat execution without factorization.
        _ => Err(ConversionError::NotYetImplemented(
            node.type_name().to_string(),
        )),
    }
}

mod access;
mod apply;
mod control_flow;
mod join;
mod operation;
mod pipeline;
mod setop;
mod traversal;
mod write;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_start() {
        let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let plan_enum = PlanNodeEnum::Start(start);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        let logical = result.unwrap();
        assert_eq!(logical.type_name(), "Start");
    }

    #[test]
    fn test_convert_filter_with_start() {
        let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let ctx = std::sync::Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let expr = linkrs_core::Expression::Variable("test".to_string());
        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let condition = linkrs_core::types::expr::contextual::ContextualExpression::new(id, ctx);

        let filter = crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
            start_enum, condition,
        )
        .expect("Filter node creation failed");

        let filter_enum = PlanNodeEnum::Filter(filter);
        let result = convert_plan(&filter_enum);
        assert!(result.is_ok());
        let logical = result.unwrap();
        assert_eq!(logical.type_name(), "Filter");
    }

    #[test]
    fn test_convert_unsupported() {
        // Argument is now supported; use a management node that remains unsupported
        // as the negative case (e.g., Tag management not yet bridged).
        let arg =
            crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(
                -1, "x",
            );
        let plan_enum = PlanNodeEnum::Argument(arg);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().type_name(), "Argument");
    }
}
