use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::common::json_to_value;

pub(crate) fn execute_in_query_call(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::InQueryCall {
        storage,
        space_name,
        func_name,
        args_json,
        yield_items,
        col_names,
        emitted,
        ..
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let names: Vec<String> = if !yield_items.is_empty() {
        yield_items.iter().map(|(alias, _)| alias.clone()).collect()
    } else if !col_names.is_empty() {
        col_names.clone()
    } else {
        vec!["result".to_string()]
    };
    // Every call result is a single column; a wider YIELD would silently
    // drop columns.
    if names.len() != 1 {
        return Err(QueryError::execution(format!(
            "CALL {} returns a single column, but YIELD lists {}",
            func_name,
            names.len()
        )));
    }
    let schema = Arc::new(Schema::new(
        names
            .iter()
            .map(|n| ColumnInfo {
                name: n.clone(),
                data_type: "string".to_string(),
            })
            .collect(),
    ));

    match func_name.as_str() {
        "db_version" => Ok(Some(super::super::make_single_row(
            schema,
            vec![Value::String(env!("CARGO_PKG_VERSION").into())],
        ))),
        "db_current_space" => Ok(Some(super::super::make_single_row(
            schema,
            vec![Value::string(space_name.clone())],
        ))),
        "db_property_graph" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for db_property_graph()".to_string(),
                    ));
                }
            };
            match reader.get_space(space_name) {
                Ok(Some(info)) => {
                    let schema_str = format!(
                        "Space: {}, Tags: {}, EdgeTypes: {}",
                        info.space_name,
                        reader.list_tags(space_name).map(|t| t.len()).unwrap_or(0),
                        reader
                            .list_edge_types(space_name)
                            .map(|e| e.len())
                            .unwrap_or(0),
                    );
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![Value::string(schema_str)],
                    )))
                }
                Ok(None) => Err(QueryError::execution(format!(
                    "Space '{}' not found",
                    space_name
                ))),
                Err(e) => Err(QueryError::execution(format!(
                    "Failed to get space info: {}",
                    e
                ))),
            }
        }
        "list_labels" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for list_labels()".to_string(),
                    ));
                }
            };
            let tags = reader
                .list_tags(space_name)
                .map_err(|e| QueryError::execution(format!("Failed to list tags: {}", e)))?;
            let rows: Vec<Vec<Value>> = tags
                .into_iter()
                .map(|t| vec![Value::string(t.tag_name)])
                .collect();
            let tag_schema = super::super::make_single_col_schema(&names[0], "string");
            Ok(Some(DataChunk::new(rows, tag_schema)))
        }
        "list_relationship_types" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for list_relationship_types()".to_string(),
                    ));
                }
            };
            let edges = reader
                .list_edge_types(space_name)
                .map_err(|e| QueryError::execution(format!("Failed to list edge types: {}", e)))?;
            let rows: Vec<Vec<Value>> = edges
                .into_iter()
                .map(|e| vec![Value::string(e.edge_type_name)])
                .collect();
            let edge_schema = super::super::make_single_col_schema(&names[0], "string");
            Ok(Some(DataChunk::new(rows, edge_schema)))
        }
        _ => {
            let registry = crate::executor::expression::functions::registry::global_registry();
            let registry = registry.read();
            let args = parse_call_args(args_json);
            let value = registry
                .execute(func_name, &args)
                .map_err(|e| QueryError::execution(format!("CALL {} failed: {}", func_name, e)))?;
            Ok(Some(super::super::make_single_row(schema, vec![value])))
        }
    }
}

pub(crate) fn execute_analyze(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::Analyze {
        storage,
        space_name,
        analyze_target,
        target_name,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match analyze_target.as_str() {
        "space" => {
            if let Some(lock) = storage {
                let reader = lock.read();
                let stats = reader.get_storage_stats();
                let schema = Arc::new(Schema::new(vec![
                    ColumnInfo {
                        name: "target".to_string(),
                        data_type: "string".to_string(),
                    },
                    ColumnInfo {
                        name: "stats".to_string(),
                        data_type: "string".to_string(),
                    },
                ]));
                Ok(Some(super::super::make_single_row(
                    schema,
                    vec![
                        Value::string(format!("space:{}", space_name)),
                        Value::string(format!("{:?}", stats)),
                    ],
                )))
            } else {
                Ok(Some(super::super::make_manage_result(
                    "analyze",
                    Some(space_name.as_str()),
                    "no-storage",
                )))
            }
        }
        "tag" | "edge" => {
            let name = target_name.as_deref().unwrap_or("");
            // Per-tag/edge statistics collection has no storage
            // implementation behind it; fail loudly instead of reporting a
            // success that collected nothing. Whole-space collection runs
            // on the ANALYZE fast path before planning reaches this operator.
            Err(QueryError::execution(format!(
                "ANALYZE for tag/edge '{}' is not supported; use ANALYZE [SPACE <name>] for whole-space statistics collection",
                name
            )))
        }
        _ => Err(QueryError::execution(format!(
            "Unsupported analyze target: {}",
            analyze_target
        ))),
    };
    result
}

fn parse_call_args(args_json: &str) -> Vec<Value> {
    let parsed: Vec<serde_json::Value> = serde_json::from_str(args_json).unwrap_or_default();
    parsed.into_iter().map(|v| json_to_value(&v)).collect()
}
