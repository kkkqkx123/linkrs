//! Build physical plans for control flow nodes (Union, Apply, etc.)

use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{ApplyKind, ApplySpec, SetSpec};
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::executor::streaming::plan::properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub fn build_control_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::Union(node) => {
            let left_phys = super::build_plan_node(node.input(), context)?;
            let right_phys = super::build_plan_node(node.union_input(), context)?;
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::UnionAll,
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Minus(node) => {
            let left_phys = super::build_plan_node(node.input(), context)?;
            let right_phys = super::build_plan_node(node.minus_input(), context)?;
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Minus,
                PhysicalProperties::single_blocking(),
            ))
        }

        PlanNodeEnum::Intersect(node) => {
            let left_phys = super::build_plan_node(node.input(), context)?;
            let right_phys = super::build_plan_node(node.intersect_input(), context)?;
            Ok(PhysicalNode::Set(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                SetSpec::Intersect,
                PhysicalProperties::single_blocking(),
            ))
        }

        PlanNodeEnum::PatternApply(apply) => {
            let left_phys = super::build_plan_node(apply.left_input(), context)?;
            let right_phys = super::build_plan_node(apply.right_input(), context)?;
            let key_exprs = apply
                .key_cols()
                .iter()
                .map(|e| {
                    e.get_expression().ok_or_else(|| {
                        PlanBuildError::expression(
                            "PatternApply",
                            node.id(),
                            format!("{:?}", e),
                            "Failed to resolve key expression",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::PatternApply {
                    key_expressions: key_exprs,
                    anti: apply.is_anti_predicate(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::RollUpApply(apply) => {
            let left_phys = super::build_plan_node(apply.left_input(), context)?;
            let right_phys = super::build_plan_node(apply.right_input(), context)?;
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::RollUpApply {
                    compare_columns: apply.compare_cols().to_vec(),
                    collect_column: apply.collect_col().map(|s| s.to_string()),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Apply(apply) => {
            let left_phys = super::build_plan_node(apply.left_input(), context)?;
            let right_phys = super::build_plan_node(apply.right_input(), context)?;
            use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind as PlanApplyKind;
            let kind = match apply.apply_kind() {
                PlanApplyKind::Semi => ApplyKind::Semi,
                PlanApplyKind::Anti => ApplyKind::Anti,
                PlanApplyKind::Single => ApplyKind::Single,
                PlanApplyKind::All => ApplyKind::All,
                PlanApplyKind::Standard => ApplyKind::Standard,
            };
            Ok(PhysicalNode::Apply(
                node.id(),
                Box::new(left_phys),
                Box::new(right_phys),
                ApplySpec::Apply {
                    kind,
                    correlated_columns: apply.correlated_cols().to_vec(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        _ => Err(super::internal_routing_error(node, "control")),
    }
}
