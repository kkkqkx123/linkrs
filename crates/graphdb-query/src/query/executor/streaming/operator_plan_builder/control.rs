use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::{ApplyKind, ApplySpec, SetSpec};
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::executor::streaming::physical_properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub fn build_control_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::Union(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.union_input();
            if left_plan.col_names() != right_plan.col_names() {
                return Err(QueryError::execution(format!(
                    "Union inputs have incompatible layouts: left={:?}, right={:?}",
                    left_plan.col_names(),
                    right_plan.col_names()
                )));
            }
            let left_phys = super::build_plan_node(left_plan, context)?;
            let right_phys = super::build_plan_node(right_plan, context)?;
            let spec = if set_node.distinct() {
                SetSpec::Union
            } else {
                SetSpec::UnionAll
            };
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                spec,
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Intersect(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.intersect_input();
            let left_phys = super::build_plan_node(left_plan, context)?;
            let right_phys = super::build_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Intersect,
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Minus(set_node) => {
            let left_plan = set_node.input();
            let right_plan = set_node.minus_input();
            let left_phys = super::build_plan_node(left_plan, context)?;
            let right_phys = super::build_plan_node(right_plan, context)?;
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Except,
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Apply(node) => {
            let left_plan = node.left_input();
            let right_plan = node.right_input();
            let left_phys = super::build_plan_node(left_plan, context)?;
            let right_phys = super::build_plan_node(right_plan, context)?;
            let kind = match node.apply_kind() {
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Standard => ApplyKind::Standard,
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Semi => ApplyKind::Semi,
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Anti => ApplyKind::Anti,
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Single => ApplyKind::Single,
                crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::All => ApplyKind::All,
            };
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::Apply {
                    kind,
                    correlated_columns: node.correlated_cols().to_vec(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::PatternApply(node) => {
            let left_plan = node.left_input();
            let right_plan = node.right_input();
            let left_phys = super::build_plan_node(left_plan, context)?;
            let right_phys = super::build_plan_node(right_plan, context)?;
            let key_expressions: Vec<Expression> = node
                .key_cols()
                .iter()
                .map(super::contextual_to_expression)
                .collect::<Result<_, _>>()?;
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::PatternApply {
                    key_expressions,
                    anti: node.is_anti_predicate(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::RollUpApply(node) => {
            let left_phys = super::build_plan_node(node.left_input(), context)?;
            let right_phys = super::build_plan_node(node.right_input(), context)?;
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::RollUpApply {
                    compare_columns: node.compare_cols().to_vec(),
                    collect_column: node.collect_col().cloned(),
                },
                PhysicalProperties::single_blocking(),
            ))
        }

        _ => Err(super::internal_routing_error(node, "control")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;
    use crate::query::validator::context::ExpressionAnalysisContext;

    fn build_union(distinct: bool) -> PhysicalNode {
        let left = PlanNodeEnum::default();
        let right = PlanNodeEnum::default();
        let union = UnionNode::new(left, right, distinct).expect("union plan should build");
        let context = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
        build_control_node(&PlanNodeEnum::Union(union), &context)
            .expect("physical union should build")
    }

    #[test]
    fn union_distinct_flag_selects_the_physical_operator() {
        assert!(matches!(
            build_union(true),
            PhysicalNode::Set(_, _, _, SetSpec::Union, _)
        ));
        assert!(matches!(
            build_union(false),
            PhysicalNode::Set(_, _, _, SetSpec::UnionAll, _)
        ));
    }
}
