//! Domain-specific physical plan lowering.
//!
//! Each module handles a family of plan nodes, converting them into
//! [`PhysicalNode`](super::physical_node::PhysicalNode) trees that can be
//! cached and repeatedly materialized into
//! [`StreamingExecutor`](super::executor::StreamingExecutor).
//!
//! Phase 2 pilot covers Source / Unary / Blocking / Join operators only
//! (everything that maps to `SourceSpec`, `UnarySpec`, `BlockingSpec`, or
//! `JoinSpec`). Other operator families (Set, Apply, Graph, Sink, Ddl,
//! Fulltext, Vector, Txn, Gather) remain on the legacy StreamingExecutor
//! build path in `builder.rs` and will be migrated in later phases.

pub mod control;
pub mod graph;
pub mod relational;
pub mod scans;
pub mod writes;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::BinaryOperator;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::JoinSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Lower any plan node into a PhysicalNode tree.
///
/// Phase 2: returns `Err` for node types not yet piloted.  The builder
/// falls back to the legacy `StreamingExecutor` path in that case.
pub fn lower_plan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    // Try domain-specific lowering
    if let Ok(phys) = scans::lower_scan_node(node, context) {
        return Ok(phys);
    }
    if let Ok(phys) = relational::lower_relational_node(node, context) {
        return Ok(phys);
    }
    if let Ok(phys) = graph::lower_graph_node(node, context) {
        return Ok(phys);
    }
    if let Ok(phys) = writes::lower_write_node(node, context) {
        return Ok(phys);
    }
    if let Ok(phys) = control::lower_control_node(node, context) {
        return Ok(phys);
    }

    // Binary/join operators (pilot)
    match node {
        PlanNodeEnum::InnerJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::InnerJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::HashInnerJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::InnerJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::LeftJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::LeftJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::HashLeftJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::LeftJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::CrossJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::CrossJoin,
            ))
        }

        PlanNodeEnum::RightJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::RightJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::FullOuterJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::FullOuterJoin { join_condition: condition },
            ))
        }

        PlanNodeEnum::SemiJoin(join_node) => {
            let left_plan = join_node.left_input();
            let right_plan = join_node.right_input();
            let left_phys = lower_plan_node(left_plan, context)?;
            let right_phys = lower_plan_node(right_plan, context)?;
            let condition = join_condition_from_keys(
                join_node.hash_keys(),
                join_node.probe_keys(),
                right_plan.col_names(),
            )?;
            Ok(PhysicalNode::Join(
                Box::new(left_phys),
                Box::new(right_phys),
                JoinSpec::SemiJoin { join_condition: condition },
            ))
        }

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
        _ => Err(QueryError::execution(format!(
            "Node not supported by PhysicalNode lowering: {}",
            node.name()
        ))),
    }
}

// ── Join helper ──

fn join_condition_from_keys(
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
