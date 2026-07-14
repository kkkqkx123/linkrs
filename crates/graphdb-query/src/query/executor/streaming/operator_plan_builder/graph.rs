use crate::core::types::expr::Expression;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{GraphSpec, JoinSpec};
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::executor::streaming::plan::properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};

pub fn build_graph_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::Expand(expand_node) => {
            let input_plan = expand_node
                .inputs()
                .first()
                .ok_or_else(|| PlanBuildError::missing_value("Expand", node.id(), "input", "Expand requires an input"))?;
            let input_phys = super::build_plan_node(input_plan, context)?;
            let edge_types = expand_node.edge_types().to_vec();
            let direction = expand_node.direction();
            let filter_expr = expand_node
                .filter()
                .map(super::contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::Expand {
                    edge_types,
                    direction,
                    filter_expr,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::ExpandAll(expand_all_node) => {
            let input_plan = expand_all_node
                .inputs()
                .first()
                .ok_or_else(|| PlanBuildError::missing_value("ExpandAll", node.id(), "input", "ExpandAll requires an input"))?;
            let input_phys = super::build_plan_node(input_plan, context)?;
            let edge_types = expand_all_node.edge_types().to_vec();
            let direction = match expand_all_node.direction().to_lowercase().as_str() {
                "out" | "outgoing" => crate::core::EdgeDirection::Out,
                "in" | "incoming" => crate::core::EdgeDirection::In,
                _ => crate::core::EdgeDirection::Both,
            };
            let filter_expr = expand_all_node
                .filter()
                .map(super::contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::ExpandAll {
                    edge_types,
                    direction,
                    filter_expr,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::Traverse(traverse_node) => {
            let input_plan = traverse_node.input();
            let input_phys = super::build_plan_node(input_plan, context)?;
            let edge_types = traverse_node.edge_types().to_vec();
            let direction = traverse_node.direction();
            let min_depth = traverse_node.min_steps();
            let max_depth = traverse_node.max_steps();
            let filter_expr = traverse_node
                .e_filter()
                .or_else(|| traverse_node.v_filter())
                .map(super::contextual_to_expression)
                .transpose()?;
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::Traverse {
                    edge_types,
                    direction,
                    min_depth,
                    max_depth,
                    filter_expr,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::BiExpand(node) => {
            let input_plan = node.left_input();
            let input_phys = super::build_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = node.left_direction();
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::BiExpand {
                    edge_types,
                    direction,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::BiTraverse(node) => {
            let input_plan = node.left_input();
            let input_phys = super::build_plan_node(input_plan, context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = node.left_direction();
            let min_depth = node.min_hops() as u32;
            let max_depth = node.max_hops() as u32;
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::BiTraverse {
                    edge_types,
                    direction,
                    min_depth,
                    max_depth,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::ShortestPath(node) => {
            if node.weight_expression().is_some() || node.heuristic_expression().is_some() {
                return Err(PlanBuildError::capability(
                    "weighted_shortest_path",
                    "Weighted shortest path is not supported by the streaming executor",
                ));
            }
            let input_phys =
                build_binary_path_input(node.id(), node.left_input(), node.right_input(), context)?;
            let edge_types = node.edge_types().to_vec();
            let target = node
                .end_vertex_ids()
                .first()
                .cloned()
                .map(Expression::Literal);
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::ShortestPath {
                    target_vertex: target,
                    edge_types,
                    direction: if node.no_reverse() {
                        crate::core::EdgeDirection::Out
                    } else {
                        crate::core::EdgeDirection::Both
                    },
                    max_depth: node.max_step(),
                    start_vertices: node.start_vertex_ids().to_vec(),
                    target_vertices: node.end_vertex_ids().to_vec(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::BFSShortest(node) => {
            let input_phys =
                build_binary_path_input(node.id(), node.left_input(), node.right_input(), context)?;
            let edge_types = node.edge_types().to_vec();
            let direction = if node.reverse() {
                crate::core::EdgeDirection::In
            } else {
                crate::core::EdgeDirection::Both
            };
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::BFSShortest {
                    target_vertex: None,
                    edge_types,
                    direction,
                    max_depth: node.steps(),
                    allow_cycles: node.with_cycle(),
                    allow_loops: node.with_loop(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::AllPaths(node) => {
            if node.max_hop() < node.min_hop() {
                return Err(PlanBuildError::missing_value(
                    "AllPaths",
                    node.id(),
                    "max_hop",
                    "AllPaths max hop must not be smaller than min hop",
                ));
            }
            let offset = usize::try_from(node.offset()).map_err(|_| {
                PlanBuildError::missing_value("AllPaths", node.id(), "offset", "AllPaths offset must be non-negative")
            })?;
            let limit = if node.limit() < 0 {
                None
            } else {
                Some(usize::try_from(node.limit()).map_err(|_| {
                    PlanBuildError::missing_value("AllPaths", node.id(), "limit", "AllPaths limit does not fit in usize")
                })?)
            };
            let input_phys =
                build_binary_path_input(node.id(), node.left_input(), node.right_input(), context)?;
            let edge_types = node.edge_types().to_vec();
            let target = node
                .end_vertex_ids()
                .first()
                .map(|id| Expression::Literal(crate::core::Value::String(id.to_string())));
            Ok(PhysicalNode::Graph(
                node.id(),
                Box::new(input_phys),
                GraphSpec::AllPaths {
                    target_vertex: target,
                    edge_types,
                    direction: crate::core::EdgeDirection::Both,
                    min_depth: node.min_hop(),
                    max_depth: node.max_hop(),
                    acyclic: node.is_acyclic(),
                    limit,
                    offset,
                    filter: node.filter().map(super::contextual_to_expression).transpose()?,
                    start_vertices: node
                        .start_vertex_ids()
                        .iter()
                        .copied()
                        .map(crate::core::Value::from)
                        .collect(),
                    target_vertices: node
                        .end_vertex_ids()
                        .iter()
                        .copied()
                        .map(crate::core::Value::from)
                        .collect(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::MultiShortestPath(_node) => {
            Err(PlanBuildError::unsupported(
                "MultiShortestPath",
                node.id(),
                "MultiShortestPath execution is not yet implemented: the planner node \
                 now carries edge_types, direction, and target_vertex_ids, but \
                 the executor spec is not wired. Full support requires weighted path \
                 and RecursiveFragmentSpec integration.",
            ))
        }

        _ => Err(super::internal_routing_error(node, "graph")),
    }
}

fn build_binary_path_input(
    node_id: i64,
    left: &PlanNodeEnum,
    right: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    Ok(PhysicalNode::Join(
        node_id,
        Box::new(super::build_plan_node(left, context)?),
        Box::new(super::build_plan_node(right, context)?),
        JoinSpec::CrossJoin,
        PhysicalProperties::single_blocking(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::query::planning::plan::core::nodes::traversal::path_algorithms::{
        AllPathsNode, BFSShortestNode, ShortestPathNode,
    };
    use crate::query::validator::context::ExpressionAnalysisContext;

    fn context() -> ExecutionContext {
        ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()))
    }

    #[test]
    fn bfs_path_preserves_both_inputs_and_configuration() {
        let mut bfs = BFSShortestNode::new(
            PlanNodeEnum::default(),
            PlanNodeEnum::default(),
            7,
            12,
            vec!["LIKES".to_string()],
            true,
        );
        bfs.set_loop(true);
        bfs.set_reverse(true);
        let physical = build_graph_node(&PlanNodeEnum::BFSShortest(bfs), &context())
            .expect("BFS physical plan should build");
        match physical {
            PhysicalNode::Graph(
                _,
                child,
                GraphSpec::BFSShortest {
                    edge_types,
                    direction,
                    max_depth,
                    allow_cycles,
                    allow_loops,
                    ..
                },
                _,
            ) => {
                assert!(matches!(*child, PhysicalNode::Join(..)));
                assert_eq!(edge_types, vec!["LIKES"]);
                assert_eq!(direction, crate::core::EdgeDirection::In);
                assert_eq!(max_depth, 12);
                assert!(allow_cycles);
                assert!(allow_loops);
            }
            other => panic!("unexpected physical node: {other:?}"),
        }
    }

    #[test]
    fn all_paths_preserves_hops_limits_and_vertex_sets() {
        let mut all_paths = AllPathsNode::new(
            PlanNodeEnum::default(),
            PlanNodeEnum::default(),
            1,
            8,
            vec!["ROAD".to_string()],
            2,
            8,
            true,
        );
        all_paths.set_limit(5);
        all_paths.set_offset(3);
        all_paths.set_start_vertex_ids(vec![crate::core::types::VertexId::from(1_i64)]);
        all_paths.set_end_vertex_ids(vec![crate::core::types::VertexId::from(9_i64)]);
        let physical = build_graph_node(&PlanNodeEnum::AllPaths(all_paths), &context())
            .expect("AllPaths physical plan should build");
        assert!(matches!(
            physical,
            PhysicalNode::Graph(
                _,
                _,
                GraphSpec::AllPaths {
                    min_depth: 2,
                    max_depth: 8,
                    limit: Some(5),
                    offset: 3,
                    ..
                },
                _
            )
        ));
    }

    #[test]
    fn weighted_shortest_path_is_rejected_before_execution() {
        let mut shortest = ShortestPathNode::new(
            PlanNodeEnum::default(),
            PlanNodeEnum::default(),
            1,
            vec!["ROAD".to_string()],
            4,
        );
        shortest.set_weight_expression("weight".to_string());
        let error = build_graph_node(&PlanNodeEnum::ShortestPath(shortest), &context())
            .expect_err("weighted path must be rejected");
        assert!(error.to_string().contains("Weighted shortest path"));
    }
}
