use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::Value;
use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::MigrateAction;

pub(super) fn execute_show_stats(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowStats {
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
        let schema = super::make_single_col_schema("message", "string");
        Ok(Some(DataChunk::new(
            vec![vec![Value::string("no storage available")]],
            schema,
        )))
    }
}

pub(super) fn execute_show_configs(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowConfigs {
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
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("module", "string"),
        vec![Value::string("graphdb")],
    )))
}

pub(super) fn execute_show_queries(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowQueries {
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
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("queries", "string"),
        vec![],
    )))
}

pub(super) fn execute_show_sessions(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowSessions {
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
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("sessions", "string"),
        vec![],
    )))
}

pub(super) fn execute_analyze(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::Analyze {
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
                Ok(Some(super::make_single_row(
                    schema,
                    vec![
                        Value::string(format!("space:{}", space_name)),
                        Value::string(format!("{:?}", stats)),
                    ],
                )))
            } else {
                Ok(Some(super::make_manage_result(
                    "analyze",
                    Some(space_name.as_str()),
                    "no-storage",
                )))
            }
        }
        "tag" | "edge" => {
            let name = target_name.as_deref().unwrap_or("");
            Ok(Some(super::make_manage_result(
                "analyze",
                Some(name),
                "executed",
            )))
        }
        _ => Err(QueryError::execution(format!(
            "Unsupported analyze target: {}",
            analyze_target
        ))),
    };
    result
}

pub(super) fn execute_migrate(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::Migrate {
        storage,
        space_name,
        action,
        migration_data: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match action {
        MigrateAction::MigrateSpace => {
            if let Some(lock) = storage {
                let writer = lock.write();
                let res = writer
                    .save_to_disk()
                    .map_err(|e| QueryError::execution(format!("Migrate failed: {}", e)));
                match res {
                    Ok(_) => Ok(Some(super::make_manage_result(
                        "migrate",
                        Some(space_name),
                        "saved",
                    ))),
                    Err(e) => Err(e),
                }
            } else {
                Ok(Some(super::make_manage_result(
                    "migrate",
                    Some(space_name),
                    "no-storage",
                )))
            }
        }
    };
    result
}
