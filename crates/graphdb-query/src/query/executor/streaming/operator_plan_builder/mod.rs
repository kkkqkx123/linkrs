//! Builds immutable physical operator plans from planner nodes.
//!
//! Every [`PlanNodeEnum`] variant maps to a [`PhysicalNode`] tree, enabling
//! the plan to be cached, EXPLAINed, and repeatedly materialized into
//! fresh [`StreamingExecutor`](super::executor::StreamingExecutor) instances.
//!
//! Each sub-module handles a family of plan nodes. The top-level
//! [`build_plan_node`] performs one exhaustive dispatch so build failures are
//! never mistaken for an unsupported node.

pub mod control;
pub mod ddl;
pub mod fulltext;
pub mod graph;
pub mod relational;
pub mod scans;
pub mod txn;
pub mod vector;
pub mod writes;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::BinaryOperator;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::JoinSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::executor::streaming::physical_properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Build a physical operator plan for any planner node.
///
/// Every known node type is handled by one of the domain-specific sub-modules.
/// Truly unsupported types (Loop, PassThrough, Select) produce an error.
pub fn build_plan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::Start(_)
        | PlanNodeEnum::Argument(_)
        | PlanNodeEnum::GetVertices(_)
        | PlanNodeEnum::GetEdges(_)
        | PlanNodeEnum::GetNeighbors(_)
        | PlanNodeEnum::ScanVertices(_)
        | PlanNodeEnum::ScanEdges(_)
        | PlanNodeEnum::EdgeIndexScan(_)
        | PlanNodeEnum::IndexScan(_) => scans::build_scan_node(node, context),

        PlanNodeEnum::Project(_)
        | PlanNodeEnum::Filter(_)
        | PlanNodeEnum::Sort(_)
        | PlanNodeEnum::Limit(_)
        | PlanNodeEnum::TopN(_)
        | PlanNodeEnum::Sample(_)
        | PlanNodeEnum::Dedup(_)
        | PlanNodeEnum::Aggregate(_)
        | PlanNodeEnum::Window(_)
        | PlanNodeEnum::DataCollect(_)
        | PlanNodeEnum::Remove(_)
        | PlanNodeEnum::Unwind(_)
        | PlanNodeEnum::Materialize(_)
        | PlanNodeEnum::Assign(_) => relational::build_relational_node(node, context),

        PlanNodeEnum::Expand(_)
        | PlanNodeEnum::ExpandAll(_)
        | PlanNodeEnum::Traverse(_)
        | PlanNodeEnum::BiExpand(_)
        | PlanNodeEnum::BiTraverse(_)
        | PlanNodeEnum::MultiShortestPath(_)
        | PlanNodeEnum::BFSShortest(_)
        | PlanNodeEnum::AllPaths(_)
        | PlanNodeEnum::ShortestPath(_) => graph::build_graph_node(node, context),

        PlanNodeEnum::Union(_)
        | PlanNodeEnum::Minus(_)
        | PlanNodeEnum::Intersect(_)
        | PlanNodeEnum::PatternApply(_)
        | PlanNodeEnum::RollUpApply(_)
        | PlanNodeEnum::Apply(_) => control::build_control_node(node, context),

        PlanNodeEnum::InsertVertices(_)
        | PlanNodeEnum::InsertEdges(_)
        | PlanNodeEnum::DeleteVertices(_)
        | PlanNodeEnum::DeleteEdges(_)
        | PlanNodeEnum::DeleteTags(_)
        | PlanNodeEnum::PipeDeleteVertices(_)
        | PlanNodeEnum::PipeDeleteEdges(_)
        | PlanNodeEnum::Update(_)
        | PlanNodeEnum::UpdateVertices(_)
        | PlanNodeEnum::UpdateEdges(_) => writes::build_write_node(node, context),

        PlanNodeEnum::SpaceManage(_)
        | PlanNodeEnum::TagManage(_)
        | PlanNodeEnum::EdgeManage(_)
        | PlanNodeEnum::IndexManage(_)
        | PlanNodeEnum::UserManage(_)
        | PlanNodeEnum::DeleteIndex(_)
        | PlanNodeEnum::ShowStats(_) => ddl::build_ddl_node(node, context),

        PlanNodeEnum::FulltextManage(_)
        | PlanNodeEnum::FulltextSearch(_)
        | PlanNodeEnum::FulltextLookup(_)
        | PlanNodeEnum::MatchFulltext(_) => fulltext::build_fulltext_node(node, context),

        PlanNodeEnum::VectorManage(_) => vector::build_vector_node(node, context),
        #[cfg(feature = "qdrant")]
        PlanNodeEnum::VectorSearch(_)
        | PlanNodeEnum::VectorLookup(_)
        | PlanNodeEnum::VectorMatch(_) => vector::build_vector_node(node, context),

        PlanNodeEnum::BeginTransaction(_) | PlanNodeEnum::Commit(_) | PlanNodeEnum::Rollback(_) => {
            txn::build_txn_node(node, context)
        }

        // Binary/join operators
        PlanNodeEnum::InnerJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::InnerJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::HashInnerJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::InnerJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::LeftJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::LeftJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::HashLeftJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::LeftJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::CrossJoin(join_node) => build_join_core(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            JoinSpec::CrossJoin,
            context,
        ),
        PlanNodeEnum::RightJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::RightJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::FullOuterJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::FullOuterJoin { join_condition: c },
            context,
        ),
        PlanNodeEnum::SemiJoin(join_node) => build_join_with_keys(
            node.id(),
            join_node.left_input(),
            join_node.right_input(),
            join_node.hash_keys(),
            join_node.probe_keys(),
            join_node.right_input().col_names(),
            |c| JoinSpec::SemiJoin { join_condition: c },
            context,
        ),

        // ── Unsupported ──────────────────────────────────
        PlanNodeEnum::Loop(_) => Err(QueryError::execution(
            "Loop plan nodes are not supported".to_string(),
        )),
        PlanNodeEnum::PassThrough(_) => Err(QueryError::execution(
            "PassThrough plan nodes are not supported".to_string(),
        )),
        PlanNodeEnum::Select(_) => Err(QueryError::execution(
            "Select plan nodes are not supported".to_string(),
        )),
        PlanNodeEnum::AppendVertices(_) => Err(QueryError::execution(
            "AppendVertices plan nodes are not supported".to_string(),
        )),
    }
}

// ── Join helpers ──

pub(super) fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, QueryError> {
    expr.get_expression().ok_or_else(|| {
        QueryError::execution("Failed to get expression from ContextualExpression".to_string())
    })
}

fn build_join_core(
    node_id: i64,
    left_plan: &PlanNodeEnum,
    right_plan: &PlanNodeEnum,
    join_spec: JoinSpec,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    let left_phys = build_plan_node(left_plan, context)?;
    let right_phys = build_plan_node(right_plan, context)?;
    Ok(PhysicalNode::Join(
        node_id,
        Box::new(left_phys),
        Box::new(right_phys),
        join_spec,
        PhysicalProperties::single_blocking(),
    ))
}

fn build_join_with_keys(
    node_id: i64,
    left_plan: &PlanNodeEnum,
    right_plan: &PlanNodeEnum,
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    right_col_names: &[String],
    make_spec: impl FnOnce(Option<Expression>) -> JoinSpec,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    let left_phys = build_plan_node(left_plan, context)?;
    let right_phys = build_plan_node(right_plan, context)?;
    let condition = join_condition_from_keys(hash_keys, probe_keys, right_col_names)?;
    Ok(PhysicalNode::Join(
        node_id,
        Box::new(left_phys),
        Box::new(right_phys),
        make_spec(condition),
        PhysicalProperties::single_blocking(),
    ))
}

pub(super) fn join_condition_from_keys(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    _right_col_names: &[String],
) -> Result<Option<Expression>, QueryError> {
    if hash_keys.is_empty() || probe_keys.is_empty() || hash_keys.len() != probe_keys.len() {
        return Ok(None);
    }
    let left_first = hash_keys[0].get_expression().ok_or_else(|| {
        QueryError::execution("Failed to resolve hash key expression".to_string())
    })?;
    let right_first = probe_keys[0].get_expression().ok_or_else(|| {
        QueryError::execution("Failed to resolve probe key expression".to_string())
    })?;
    let mut condition = Expression::Binary {
        left: Box::new(left_first),
        op: BinaryOperator::Equal,
        right: Box::new(right_first),
    };
    for i in 1..hash_keys.len() {
        let left = hash_keys[i].get_expression().ok_or_else(|| {
            QueryError::execution("Failed to resolve hash key expression".to_string())
        })?;
        let right = probe_keys[i].get_expression().ok_or_else(|| {
            QueryError::execution("Failed to resolve probe key expression".to_string())
        })?;
        let eq = Expression::Binary {
            left: Box::new(left),
            op: BinaryOperator::Equal,
            right: Box::new(right),
        };
        condition = Expression::Binary {
            left: Box::new(condition),
            op: BinaryOperator::And,
            right: Box::new(eq),
        };
    }
    Ok(Some(condition))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;
    use crate::query::validator::context::ExpressionAnalysisContext;

    #[test]
    fn domain_build_errors_are_not_replaced_by_unsupported_errors() {
        let limit =
            LimitNode::new(PlanNodeEnum::default(), -1, 10).expect("logical limit should build");
        let context = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
        let error = build_plan_node(&PlanNodeEnum::Limit(limit), &context)
            .expect_err("negative limit offset must fail physical planning");
        let message = error.to_string();
        assert!(message.contains("Limit offset must fit in u32"));
        assert!(!message.contains("not supported"));
    }
}
