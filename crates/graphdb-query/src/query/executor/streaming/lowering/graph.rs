use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::GraphSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};

fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, QueryError> {
    expr.get_expression().ok_or_else(|| {
        QueryError::execution("Failed to get expression from ContextualExpression".to_string())
    })
}

pub fn lower_graph_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::Expand(expand_node) => {
            let input_plan = expand_node
                .inputs()
                .first()
                .ok_or_else(|| QueryError::execution("Expand requires an input".to_string()))?;
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = expand_node.edge_types().to_vec();
            let direction = expand_node.direction();
            let filter_expr = expand_node
                .filter()
                .map(contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::Expand {
                    edge_types,
                    direction,
                    filter_expr,
                },
            ))
        }

        PlanNodeEnum::ExpandAll(expand_all_node) => {
            let input_plan = expand_all_node.inputs().first().ok_or_else(|| {
                QueryError::execution("ExpandAll requires an input".to_string())
            })?;
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = expand_all_node.edge_types().to_vec();
            let direction = match expand_all_node.direction().to_lowercase().as_str() {
                "out" | "outgoing" => crate::core::EdgeDirection::Out,
                "in" | "incoming" => crate::core::EdgeDirection::In,
                _ => crate::core::EdgeDirection::Both,
            };
            let filter_expr = expand_all_node
                .filter()
                .map(contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::ExpandAll {
                    edge_types,
                    direction,
                    filter_expr,
                },
            ))
        }

        PlanNodeEnum::Traverse(traverse_node) => {
            let input_plan = traverse_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = traverse_node.edge_types().to_vec();
            let direction = traverse_node.direction();
            let min_depth = traverse_node.min_steps();
            let max_depth = traverse_node.max_steps();
            let filter_expr = traverse_node
                .e_filter()
                .or_else(|| traverse_node.v_filter())
                .map(contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::Traverse {
                    edge_types,
                    direction,
                    min_depth,
                    max_depth,
                    filter_expr,
                },
            ))
        }

        PlanNodeEnum::BiExpand(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = node.left_direction();
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::BiExpand {
                    edge_types,
                    direction,
                },
            ))
        }

        PlanNodeEnum::BiTraverse(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = node.left_direction();
            let min_depth = node.min_hops() as u32;
            let max_depth = node.max_hops() as u32;
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::BiTraverse {
                    edge_types,
                    direction,
                    min_depth,
                    max_depth,
                },
            ))
        }

        PlanNodeEnum::ShortestPath(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let target = node
                .end_vertex_ids()
                .first()
                .cloned()
                .map(Expression::Literal);
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::ShortestPath {
                    target_vertex: target,
                    edge_types,
                    direction: crate::core::EdgeDirection::Both,
                },
            ))
        }

        PlanNodeEnum::BFSShortest(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = if node.reverse() {
                crate::core::EdgeDirection::In
            } else {
                crate::core::EdgeDirection::Both
            };
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::BFSShortest {
                    target_vertex: None,
                    edge_types,
                    direction,
                },
            ))
        }

        PlanNodeEnum::AllPaths(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let target = node
                .end_vertex_ids()
                .first()
                .map(|id| Expression::Literal(crate::core::Value::String(id.to_string())));
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::AllPaths {
                    target_vertex: target,
                    edge_types,
                    direction: crate::core::EdgeDirection::Both,
                },
            ))
        }

        PlanNodeEnum::MultiShortestPath(node) => {
            let input_plan = node.left_input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Graph(
                Box::new(input_phys),
                GraphSpec::MultiShortestPath {
                    target_vertices: Vec::new(),
                    edge_types: Vec::new(),
                    direction: crate::core::EdgeDirection::Both,
                },
            ))
        }

        _ => Err(QueryError::execution(format!(
            "lowering::graph does not handle node type: {}",
            node.name()
        ))),
    }
}
