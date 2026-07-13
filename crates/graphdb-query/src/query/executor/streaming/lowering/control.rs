use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::{ApplySpec, SetSpec};
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, QueryError> {
    expr.get_expression().ok_or_else(|| {
        QueryError::execution("Failed to get expression from ContextualExpression".to_string())
    })
}

pub fn lower_control_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::Union(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.union_input();
            let left_phys = super::lower_plan_node(left_plan, context)?;
            let right_phys = super::lower_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Set(
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Union,
            ))
        }

        PlanNodeEnum::Intersect(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.intersect_input();
            let left_phys = super::lower_plan_node(left_plan, context)?;
            let right_phys = super::lower_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Set(
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Intersect,
            ))
        }

        PlanNodeEnum::Minus(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.minus_input();
            let left_phys = super::lower_plan_node(left_plan, context)?;
            let right_phys = super::lower_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Set(
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Except,
            ))
        }

        PlanNodeEnum::Apply(node) => {
            let left_plan = node.left_input();
            let right_plan = node.right_input();
            let left_phys = super::lower_plan_node(left_plan, context)?;
            let right_phys = super::lower_plan_node(right_plan, context)?;
            let kind_label = match node.apply_kind() {
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Standard => "standard",
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Semi => "semi",
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Anti => "anti",
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Single => "single",
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::All => "all",
            };
            Ok(PhysicalNode::Apply(
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::Apply {
                    apply_expression: Expression::Literal(Value::String(
                        kind_label.to_string(),
                    )),
                },
            ))
        }

        PlanNodeEnum::PatternApply(node) => {
            let left_plan = node.left_input();
            let right_plan = node.right_input();
            let left_phys = super::lower_plan_node(left_plan, context)?;
            let right_phys = super::lower_plan_node(right_plan, context)?;
            let key_exprs: Vec<Expression> = node
                .key_cols()
                .iter()
                .filter_map(|c| contextual_to_expression(c).ok())
                .collect();
            let pattern_expr = if key_exprs.is_empty() {
                Expression::Literal(Value::Bool(true))
            } else if key_exprs.len() == 1 {
                key_exprs.into_iter().next().unwrap()
            } else {
                Expression::Literal(Value::String("correlated".to_string()))
            };
            Ok(PhysicalNode::Apply(
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::PatternApply { pattern: pattern_expr },
            ))
        }

        _ => Err(QueryError::execution(format!(
            "lowering::control does not handle node type: {}",
            node.name()
        ))),
    }
}
