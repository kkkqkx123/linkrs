use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    VectorManageCommand, VectorSpec
};

fn vector_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
) -> VectorManageCommand {
    use crate::executor::streaming::operators::spec::VectorManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode::*;
    match node {
        Create(n) => VectorManageCommand::Create {
            index_name: n.index_name.clone(),
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            vector_size: n.vector_size,
            distance: n.distance,
            space_id: n.space_id,
            hnsw_m: n.hnsw_m,
            hnsw_ef_construct: n.hnsw_ef_construct,
            quantization: n.quantization,
            quantile: n.quantile,
            compression: n.compression,
            always_ram: n.always_ram,
        },
        Drop(n) => VectorManageCommand::Drop {
            index_name: n.index_name.clone(),
            if_exists: n.if_exists,
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
        },
        Show(n) => VectorManageCommand::Show {
            pattern: n.pattern.clone(),
            from_schema: n.from_schema.clone(),
        },
        Describe(n) => VectorManageCommand::Describe {
            index_name: n.index_name.clone(),
        },
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_vector_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: vector_manage_to_command(node),
    })
}

#[cfg(feature = "vector")]
pub(in crate::executor::streaming::plan::arena_builder) fn build_simvec_spec(
    node: &crate::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    let (query_vector, query_text) = vector_query_to_vec(&node.query, exec_ctx)?;
    Ok(VectorSpec::VectorSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        index_name: node.index_name.clone(),
        query_vector,
        query_text,
        top_k: node.limit as u32,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        threshold: node.threshold,
        filter: node.filter.clone(),
        offset: node.offset,
        ryw_config: exec_ctx.ryw_config,
    })
}

#[cfg(feature = "vector")]
pub(in crate::executor::streaming::plan::arena_builder) fn build_vector_lookup_spec(
    node: &crate::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    // LOOKUP VECTOR resolves to the same index location as SEARCH VECTOR and
    // executes through the identical search path; the query expression keeps
    // its semantics (converted here exactly like a search query vector).
    let (query_vector, query_text) = vector_query_to_vec(&node.query, exec_ctx)?;
    Ok(VectorSpec::VectorLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        index_name: node.index_name.clone(),
        query_vector,
        query_text,
        top_k: node.limit as u32,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        ryw_config: exec_ctx.ryw_config,
    })
}

#[cfg(feature = "vector")]
pub(in crate::executor::streaming::plan::arena_builder) fn build_vector_match_spec(
    node: &crate::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    let (query_vector, query_text) = vector_query_to_vec(&node.query, exec_ctx)?;
    Ok(VectorSpec::VectorMatch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        pattern: node.pattern.clone(),
        field: node.field.clone(),
        query_vector,
        query_text,
        threshold: node.threshold,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        space_id: node.space_id,
        ryw_config: exec_ctx.ryw_config,
    })
}

#[cfg(feature = "vector")]

fn vector_query_to_vec(
    expr: &crate::parser::ast::vector::VectorQueryExpr,
    exec_ctx: &ExecutionContext,
) -> Result<(Vec<f32>, Option<String>), PlanBuildError> {
    match expr.query_type {
        crate::parser::ast::vector::VectorQueryType::Vector => {
            let vec: Vec<f32> = serde_json::from_str(&expr.query_data).unwrap_or_default();
            Ok((vec, None))
        }
        crate::parser::ast::vector::VectorQueryType::Text => {
            // Text queries require embedding at execution time (async);
            // return an empty placeholder vector so the spec can carry the
            // raw text forward for deferred resolution in VectorOperator.
            Ok((vec![], Some(expr.query_data.clone())))
        }
        crate::parser::ast::vector::VectorQueryType::Parameter => {
            let param_name = expr.query_data.trim_start_matches('$');
            let val = exec_ctx.get_param(param_name).ok_or_else(|| {
                PlanBuildError::capability(
                    "parameterized-vector-query",
                    format!("Parameter '${}' is not bound", param_name),
                )
            })?;
            let vec = resolve_param_to_vector(val, param_name)?;
            Ok((vec, None))
        }
    }
}

/// Resolve a `Value` to a `Vec<f32>` for vector query parameters.
///
/// Supports:
/// - `Value::String("[0.1, 0.2, ...]")` — JSON array string
/// - `Value::List(Vec<Value>)` — list of numeric values
#[cfg(feature = "vector")]
fn resolve_param_to_vector(
    val: &linkrs_core::Value,
    param_name: &str,
) -> Result<Vec<f32>, PlanBuildError> {
    match val {
        linkrs_core::Value::String(s) => serde_json::from_str(s).map_err(|_| {
            PlanBuildError::capability(
                "parameterized-vector-query",
                format!(
                    "Parameter '${}' value is not a valid JSON vector: '{}'",
                    param_name, s
                ),
            )
        }),
        linkrs_core::Value::List(list) => list
            .iter()
            .map(|v| match v {
                linkrs_core::Value::Float(f) => Ok(*f),
                linkrs_core::Value::Int(i) => Ok(*i as f32),
                linkrs_core::Value::Double(d) => Ok(*d as f32),
                _ => Err(PlanBuildError::capability(
                    "parameterized-vector-query",
                    format!(
                        "Parameter '${}' element is not numeric: {:?}",
                        param_name, v
                    ),
                )),
            })
            .collect(),
        _ => Err(PlanBuildError::capability(
            "parameterized-vector-query",
            format!(
                "Parameter '${}' value is not a string or list: {:?}",
                param_name, val
            ),
        )),
    }
}
