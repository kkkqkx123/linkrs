//! Conversion from `PlanNodeEnum` to `LogicalNodeEnum`.
//!
//! This module implements the physical-to-logical compatibility bridge used
//! only when a planner did not emit a native logical tree. Factorized
//! planners attach the logical tree directly, so this reverse pass runs
//! solely for statements that stay physical-only by design.
//! - Physical operators (IndexScan, InnerJoin, LeftJoin)
//!   are mapped to their logical equivalents.
//! - Algorithm, search, and transaction operators are mapped field for field.
//! - Management, statistics, system, and direct control-flow nodes return
//!   an explicit exemption and keep flat execution without factorization.

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

/// Errors that can occur during physical-to-logical conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// The plan contains a node type not yet supported in conversion.
    NotYetImplemented(String),
    /// The plan node stays physical-only by design and needs no logical mirror.
    Exempt(String),
}

impl ConversionError {
    /// Whether the failure is an explicit physical-only exemption.
    pub fn is_exempt(&self) -> bool {
        matches!(self, Self::Exempt(_))
    }
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotYetImplemented(name) => {
                write!(f, "conversion not yet implemented for: {}", name)
            }
            Self::Exempt(name) => {
                write!(f, "physical-only by design, no logical mirror: {}", name)
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
/// Statements that remain physical-only by design return `Exempt`
/// and keep flat execution without factorization. Only legacy aggregate
/// nodes without expression group keys return `NotYetImplemented`.
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
        PlanNodeEnum::BeginTransaction(n) => control_flow::convert_begin_transaction(n),
        PlanNodeEnum::Commit(n) => control_flow::convert_commit(n),
        PlanNodeEnum::Rollback(n) => control_flow::convert_rollback(n),
        // === Pipeline nodes ===
        PlanNodeEnum::Assign(n) => pipeline::convert_assign(n),
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
        // === Algorithm nodes ===
        PlanNodeEnum::MultiShortestPath(n) => traversal::convert_multi_shortest_path(n),
        PlanNodeEnum::BFSShortest(n) => traversal::convert_bfs_shortest(n),
        PlanNodeEnum::AllPaths(n) => traversal::convert_all_paths(n),
        PlanNodeEnum::ShortestPath(n) => traversal::convert_shortest_path(n),
        // === Search nodes ===
        PlanNodeEnum::FulltextSearch(n) => access::convert_fulltext_search(n),
        PlanNodeEnum::FulltextLookup(n) => access::convert_fulltext_lookup(n),
        PlanNodeEnum::MatchFulltext(n) => access::convert_match_fulltext(n),
        #[cfg(feature = "vector")]
        PlanNodeEnum::VectorSearch(n) => access::convert_vector_search(n),
        #[cfg(feature = "vector")]
        PlanNodeEnum::VectorLookup(n) => access::convert_vector_lookup(n),
        #[cfg(feature = "vector")]
        PlanNodeEnum::VectorMatch(n) => access::convert_vector_match(n),
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
        // === Physical-only exemptions ===
        // Management, statistics, system, and direct control-flow nodes carry
        // no logical mirror by design. New operators must add an explicit arm
        // above (bridge) or here (exempt); the wildcard below stays unreachable.
        PlanNodeEnum::RecursiveCte(_)
        | PlanNodeEnum::Savepoint(_)
        | PlanNodeEnum::ReleaseSavepoint(_)
        | PlanNodeEnum::SpaceManage(_)
        | PlanNodeEnum::TagManage(_)
        | PlanNodeEnum::EdgeManage(_)
        | PlanNodeEnum::IndexManage(_)
        | PlanNodeEnum::UserManage(_)
        | PlanNodeEnum::FulltextManage(_)
        | PlanNodeEnum::VectorManage(_)
        | PlanNodeEnum::ShowStats(_)
        | PlanNodeEnum::ShowConfigs(_)
        | PlanNodeEnum::ShowQueries(_)
        | PlanNodeEnum::ShowSessions(_)
        | PlanNodeEnum::ShowFunctions(_)
        | PlanNodeEnum::ShowGraphs(_)
        | PlanNodeEnum::ShowMacros(_)
        | PlanNodeEnum::ShowAttachedDatabases(_)
        | PlanNodeEnum::ShowExtensions(_)
        | PlanNodeEnum::LoadFrom(_)
        | PlanNodeEnum::InQueryCall(_)
        | PlanNodeEnum::MacroManage(_)
        | PlanNodeEnum::TypeManage(_)
        | PlanNodeEnum::SequenceManage(_) => {
            Err(ConversionError::Exempt(node.type_name().to_string()))
        }
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
        let arg =
            crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(
                -1, "x",
            );
        let plan_enum = PlanNodeEnum::Argument(arg);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().type_name(), "Argument");
    }

    #[test]
    fn test_convert_exempt_management() {
        let node =
            crate::planning::plan::core::nodes::control_flow::control_flow_node::SavepointNode::new(
                1,
                "sp1".to_string(),
            );
        let plan_enum = PlanNodeEnum::Savepoint(node);
        let result = convert_plan(&plan_enum);
        let err = result.expect_err("savepoint stays exempt");
        assert!(err.is_exempt());
    }

    #[test]
    fn test_convert_transaction_round_trip() {
        use crate::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode;
        let node = BeginTransactionNode::new(1).with_read_only(true);
        let plan_enum = PlanNodeEnum::BeginTransaction(node);
        let logical = convert_plan(&plan_enum).expect("txn bridges");
        assert!(matches!(
            logical,
            crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum::BeginTransaction(_)
        ));
    }

    #[test]
    fn test_convert_legacy_aggregate_guard() {
        use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
        let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let node = AggregateNode::new(
            PlanNodeEnum::Start(start),
            vec!["a".to_string()],
            vec![linkrs_core::types::operators::AggregateFunction::Count],
        )
        .expect("aggregate builds");
        let plan_enum = PlanNodeEnum::Aggregate(node);
        let err = convert_plan(&plan_enum).expect_err("legacy aggregate without exprs errors");
        assert!(!err.is_exempt());
    }
}
