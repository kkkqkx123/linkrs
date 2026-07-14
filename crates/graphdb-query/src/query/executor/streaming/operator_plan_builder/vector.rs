#[cfg(feature = "qdrant")]
use crate::core::types::expr::Expression;
#[cfg(feature = "qdrant")]
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::VectorSpec;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_vector_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::VectorManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            VectorSpec::VectorManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalNode::Vector,
        )),

        #[cfg(feature = "qdrant")]
        PlanNodeEnum::VectorSearch(search_node) => {
            let query_vec = vector_query_to_vec(&search_node.query);
            Ok(super::build_leaf_command(
                node.id(),
                VectorSpec::VectorSearch {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id: search_node.space_id,
                    index_name: search_node.index_name.clone(),
                    query_vector: query_vec,
                    top_k: search_node.limit as u32,
                    tag_name: search_node.tag_name.clone(),
                    field_name: search_node.field_name.clone(),
                },
                PhysicalNode::Vector,
            ))
        }

        #[cfg(feature = "qdrant")]
        PlanNodeEnum::VectorLookup(lookup_node) => Ok(super::build_leaf_command(
            node.id(),
            VectorSpec::VectorLookup {
                space_name: context.space_name.clone().unwrap_or_default(),
                index_name: lookup_node.index_name.clone(),
                lookup_key: Expression::Literal(Value::String(
                    lookup_node.query.query_data.clone(),
                )),
            },
            PhysicalNode::Vector,
        )),

        #[cfg(feature = "qdrant")]
        PlanNodeEnum::VectorMatch(match_node) => {
            let query_vec = vector_query_to_vec(&match_node.query);
            Ok(super::build_leaf_command(
                node.id(),
                VectorSpec::VectorMatch {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    pattern: match_node.pattern.clone(),
                    field: match_node.field.clone(),
                    query_vector: query_vec,
                    threshold: match_node.threshold,
                    tag_name: match_node.tag_name.clone(),
                    field_name: match_node.field_name.clone(),
                    space_id: match_node.space_id,
                },
                PhysicalNode::Vector,
            ))
        }

        _ => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            format!(
                "operator_plan_builder::vector does not handle node type: {}",
                node.name()
            ),
        )),
    }
}

#[cfg(feature = "qdrant")]
fn vector_query_to_vec(expr: &crate::query::parser::ast::vector::VectorQueryExpr) -> Vec<f32> {
    serde_json::from_str(&expr.query_data).unwrap_or_default()
}
