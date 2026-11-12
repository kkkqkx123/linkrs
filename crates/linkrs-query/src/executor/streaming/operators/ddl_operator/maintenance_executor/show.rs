use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use linkrs_core::error::QueryError;
use linkrs_core::Value;


pub(crate) fn execute_show_stats(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowStats {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    if let Some(storage_lock) = storage {
        let reader = storage_lock.read();
        let stats = reader.get_storage_stats();

        let schema = Arc::new(Schema::new(vec![
            ColumnInfo {
                name: "metric".to_string(),
                data_type: "string".to_string(),
            },
            ColumnInfo {
                name: "value".to_string(),
                data_type: "string".to_string(),
            },
        ]));
        let rows = vec![
            vec![
                Value::string("total_vertices"),
                Value::BigInt(stats.total_vertices as i64),
            ],
            vec![
                Value::string("total_edges"),
                Value::BigInt(stats.total_edges as i64),
            ],
            vec![
                Value::string("total_spaces"),
                Value::BigInt(stats.total_spaces as i64),
            ],
            vec![
                Value::string("total_tags"),
                Value::BigInt(stats.total_tags as i64),
            ],
            vec![
                Value::string("total_edge_types"),
                Value::BigInt(stats.total_edge_types as i64),
            ],
            vec![
                Value::string("total_size_bytes"),
                Value::BigInt(stats.total_size_bytes as i64),
            ],
            vec![
                Value::string("data_size_bytes"),
                Value::BigInt(stats.data_size_bytes as i64),
            ],
            vec![
                Value::string("index_size_bytes"),
                Value::BigInt(stats.index_size_bytes as i64),
            ],
        ];
        Ok(Some(DataChunk::new(rows, schema)))
    } else {
        let schema = super::super::make_single_col_schema("message", "string");
        Ok(Some(DataChunk::new(
            vec![vec![Value::string("no storage available")]],
            schema,
        )))
    }
}

pub(crate) fn execute_show_configs(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowConfigs {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    Ok(Some(super::super::make_single_row(
        super::super::make_single_col_schema("module", "string"),
        vec![Value::string("linkrs")],
    )))
}

pub(crate) fn execute_show_queries(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowQueries { emitted, .. } = &mut op.kind else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    // Session-scoped query tracking lives in the server session service;
    // the engine has no registry access here, so report the boundary
    // instead of an empty placeholder that reads as "no queries".
    Err(QueryError::execution(
        "SHOW QUERIES is served by the network service from live query tracking and is not available in this execution path; use the unary query endpoint"
            .to_string(),
    ))
}

pub(crate) fn execute_show_sessions(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowSessions { emitted, .. } = &mut op.kind else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    // Session state lives in the server session manager; the engine has
    // no access to it, so report the boundary instead of an empty
    // placeholder that reads as "no sessions".
    Err(QueryError::execution(
        "SHOW SESSIONS is served by the network service from the live session registry and is not available in this execution path; use the unary query endpoint"
            .to_string(),
    ))
}

pub(crate) fn execute_show_functions(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowFunctions {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let registry = crate::executor::expression::functions::registry::global_registry();
    let registry = registry.read();
    let names = registry.function_names();
    let schema = super::super::make_single_col_schema("functions", "string");
    let rows: Vec<Vec<Value>> = names
        .into_iter()
        .map(|name| vec![Value::string(name)])
        .collect();
    Ok(Some(DataChunk::new(rows, schema)))
}

pub(crate) fn execute_show_graphs(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::ShowGraphs {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let schema = super::super::make_single_col_schema("graphs", "string");
    if let Some(storage_lock) = storage {
        let reader = storage_lock.read();
        let spaces = reader
            .list_spaces()
            .map_err(|e| QueryError::execution(e.to_string()))?;
        let rows: Vec<Vec<Value>> = spaces
            .into_iter()
            .map(|s| vec![Value::string(s.space_name)])
            .collect();
        Ok(Some(DataChunk::new(rows, schema)))
    } else {
        Ok(Some(DataChunk::new(vec![], schema)))
    }
}

pub(crate) fn execute_show_macros(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let emitted = match &mut op.kind {
        super::super::DdlOperatorKind::ShowMacros { emitted, .. } => emitted,
        _ => return Ok(None),
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let defs = op
        .runtime
        .as_ref()
        .and_then(|rt| rt.macro_manager())
        .map(|manager| manager.list_macros())
        .unwrap_or_default();

    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "params".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "body".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    let rows = defs
        .iter()
        .map(|def| {
            let params = def
                .params
                .iter()
                .map(|p| match &p.default {
                    Some(default) => {
                        format!("{} = {}", p.name, default.to_expression_string())
                    }
                    None => p.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                Value::string(&def.name),
                Value::string(params),
                Value::string(def.body.to_expression_string()),
            ]
        })
        .collect::<Vec<_>>();
    Ok(Some(DataChunk::new(rows, schema)))
}

pub(crate) fn execute_show_attached_databases(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let emitted = match &mut op.kind {
        super::super::DdlOperatorKind::ShowAttachedDatabases { emitted, .. } => emitted,
        _ => return Ok(None),
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "path".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "db_type".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    let rows = crate::attached::list_attached_databases()
        .into_iter()
        .map(|entry| {
            vec![
                Value::string(entry.alias),
                Value::string(entry.path),
                Value::string(entry.db_type.unwrap_or_default()),
            ]
        })
        .collect::<Vec<_>>();
    Ok(Some(DataChunk::new(rows, schema)))
}

pub(crate) fn execute_show_extensions(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let emitted = match &mut op.kind {
        super::super::DdlOperatorKind::ShowExtensions { emitted, .. } => emitted,
        _ => return Ok(None),
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "path".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "description".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    let registry = crate::executor::expression::functions::registry::global_registry();
    let registry = registry.read();
    let rows = registry
        .list_dynamic_udfs()
        .into_iter()
        .map(|info| {
            vec![
                Value::string(info.name),
                Value::string(info.path),
                Value::string(info.description),
            ]
        })
        .collect::<Vec<_>>();
    Ok(Some(DataChunk::new(rows, schema)))
}
