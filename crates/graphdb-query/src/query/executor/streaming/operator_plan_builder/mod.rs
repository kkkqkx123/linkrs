//! Builds immutable physical operator plans from planner nodes.
//!
//! Every [`PlanNodeEnum`] variant maps to a [`PhysicalNode`] tree, enabling
//! the plan to be cached, EXPLAINed, and repeatedly materialized into
//! fresh [`StreamingExecutor`](super::executor::StreamingExecutor) instances.
//!
//! Each sub-module handles a family of plan nodes. The top-level
//! [`build_plan_node`] performs one exhaustive dispatch so build failures are
//! never mistaken for an unsupported node.

pub mod capability_matrix;
pub mod control;
pub mod ddl;
pub mod fulltext;
pub mod graph;
pub mod relational;
pub mod scans;
pub mod txn;
pub mod vector;
pub mod writes;

use crate::core::types::expr::Expression;
use crate::core::types::operators::BinaryOperator;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{JoinSpec, SourceSpec};
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::executor::streaming::plan::properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Build a physical operator plan for any planner node.
///
/// Every known node type is handled by one of the domain-specific sub-modules.
/// Truly unsupported types (Loop, PassThrough, Select) produce a structured
/// [`PlanBuildError::UnsupportedNode`] error.
pub fn build_plan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
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
        | PlanNodeEnum::BiTraverse(_) => graph::build_graph_node(node, context),

        PlanNodeEnum::MultiShortestPath(_)
        | PlanNodeEnum::BFSShortest(_)
        | PlanNodeEnum::AllPaths(_)
        | PlanNodeEnum::ShortestPath(_) => graph::build_recursive_fragment_node(node, context),

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
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::InnerJoin { join_condition: c }),
            },
            context,
        ),
        PlanNodeEnum::HashInnerJoin(join_node) => build_join_with_keys(
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::InnerJoin { join_condition: c }),
            },
            context,
        ),
        PlanNodeEnum::LeftJoin(join_node) => build_join_with_keys(
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::LeftJoin { join_condition: c }),
            },
            context,
        ),
        PlanNodeEnum::HashLeftJoin(join_node) => build_join_with_keys(
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::LeftJoin { join_condition: c }),
            },
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
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::RightJoin { join_condition: c }),
            },
            context,
        ),
        PlanNodeEnum::FullOuterJoin(join_node) => build_join_with_keys(
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::FullOuterJoin { join_condition: c }),
            },
            context,
        ),
        PlanNodeEnum::SemiJoin(join_node) => build_join_with_keys(
            JoinConfig {
                node_id: node.id(),
                left_plan: join_node.left_input(),
                right_plan: join_node.right_input(),
                hash_keys: join_node.hash_keys(),
                probe_keys: join_node.probe_keys(),
                right_col_names: join_node.right_input().col_names(),
                make_spec: Box::new(|c| JoinSpec::SemiJoin { join_condition: c }),
            },
            context,
        ),

        // Unsupported: these node types have no physical executor implementation.
        PlanNodeEnum::Loop(_) => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            "Loop plan nodes are not supported",
        )),
        PlanNodeEnum::PassThrough(_) => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            "PassThrough plan nodes are not supported",
        )),
        PlanNodeEnum::Select(_) => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            "Select plan nodes are not supported",
        )),
        PlanNodeEnum::AppendVertices(_) => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            "AppendVertices plan nodes are not supported",
        )),
    }
}

// ── Join helpers ──

pub(super) fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, PlanBuildError> {
    expr.get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "ContextualExpression",
            0,
            format!("{:?}", expr),
            "Failed to get expression from ContextualExpression",
        )
    })
}

/// Synthetic node ID for the no-op Start source used as the child of
/// leaf command operators. Chosen from the reserved sentinel range
/// to avoid collisions with real plan node IDs.
const SYNTHETIC_START_NODE_ID: i64 = i64::MIN + 4;

pub(super) fn single_start_source() -> Box<PhysicalNode> {
    Box::new(PhysicalNode::Source(
        SYNTHETIC_START_NODE_ID,
        SourceSpec::Start,
        PhysicalProperties::single_streaming(),
    ))
}

pub(super) fn build_leaf_command<Spec>(
    id: i64,
    spec: Spec,
    ctor: fn(i64, Box<PhysicalNode>, Spec, PhysicalProperties) -> PhysicalNode,
) -> PhysicalNode {
    ctor(
        id,
        single_start_source(),
        spec,
        PhysicalProperties::single_blocking(),
    )
}

pub(super) fn internal_routing_error(node: &PlanNodeEnum, builder: &str) -> PlanBuildError {
    PlanBuildError::unsupported(
        node.name(),
        node.id(),
        format!(
            "Internal routing error: node was incorrectly routed to {} builder",
            builder
        ),
    )
}

fn build_join_core(
    node_id: i64,
    left_plan: &PlanNodeEnum,
    right_plan: &PlanNodeEnum,
    join_spec: JoinSpec,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    let left_phys = build_plan_node(left_plan, context)?;
    let right_phys = build_plan_node(right_plan, context)?;
    Ok(PhysicalNode::Join(
        node_id,
        Box::new(left_phys),
        Box::new(right_phys),
        join_spec,
        PhysicalProperties::single_blocking_with_budget(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_join_with_keys(
    config: JoinConfig,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    let left_phys = build_plan_node(config.left_plan, context)?;
    let right_phys = build_plan_node(config.right_plan, context)?;
    let condition = join_condition_from_keys(config.hash_keys, config.probe_keys, config.right_col_names)?;
    Ok(PhysicalNode::Join(
        config.node_id,
        Box::new(left_phys),
        Box::new(right_phys),
        (config.make_spec)(condition),
        PhysicalProperties::single_blocking_with_budget(),
    ))
}

struct JoinConfig<'a> {
    node_id: i64,
    left_plan: &'a PlanNodeEnum,
    right_plan: &'a PlanNodeEnum,
    hash_keys: &'a [crate::core::types::expr::ContextualExpression],
    probe_keys: &'a [crate::core::types::expr::ContextualExpression],
    right_col_names: &'a [String],
    make_spec: Box<dyn FnOnce(Option<Expression>) -> JoinSpec + 'a>,
}

pub(super) fn join_condition_from_keys(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    _right_col_names: &[String],
) -> Result<Option<Expression>, PlanBuildError> {
    if hash_keys.is_empty() || probe_keys.is_empty() || hash_keys.len() != probe_keys.len() {
        return Ok(None);
    }
    let left_first = hash_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", hash_keys[0]),
            "Failed to resolve hash key expression",
        )
    })?;
    let right_first = probe_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", probe_keys[0]),
            "Failed to resolve probe key expression",
        )
    })?;
    let mut condition = Expression::Binary {
        left: Box::new(left_first),
        op: BinaryOperator::Equal,
        right: Box::new(right_first),
    };
    for i in 1..hash_keys.len() {
        let left = hash_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", hash_keys[i]),
                "Failed to resolve hash key expression",
            )
        })?;
        let right = probe_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", probe_keys[i]),
                "Failed to resolve probe key expression",
            )
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
    use crate::query::executor::build_error::PlanBuildError;
    use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;

    #[test]
    fn domain_build_errors_are_not_replaced_by_unsupported_errors() {
        let limit =
            LimitNode::new(PlanNodeEnum::default(), -1, 10).expect("logical limit should build");
        let context = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
        let error = build_plan_node(&PlanNodeEnum::Limit(limit), &context)
            .expect_err("negative limit offset must fail physical planning");
        match &error {
            PlanBuildError::MissingRequiredValue { .. } => {}
            _ => panic!("expected MissingRequiredValue, got: {error}"),
        }
        let message = error.to_string();
        assert!(message.contains("Limit offset must fit in u32"));
        assert!(!message.contains("not supported"));
    }
}
