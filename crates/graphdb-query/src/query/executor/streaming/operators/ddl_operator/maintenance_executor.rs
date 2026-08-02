use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::spec::MigrateAction;
use crate::storage::QueryStorage;

pub(super) fn execute_show_stats(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    base.lifecycle.mark_closed();

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
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    _space_name: &str,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    base.lifecycle.mark_closed();
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("module", "string"),
        vec![Value::string("graphdb")],
    )))
}

pub(super) fn execute_show_queries(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    _space_name: &str,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    base.lifecycle.mark_closed();
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("queries", "string"),
        vec![],
    )))
}

pub(super) fn execute_show_sessions(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    _space_name: &str,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    base.lifecycle.mark_closed();
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("sessions", "string"),
        vec![],
    )))
}

pub(super) fn execute_analyze(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    analyze_target: &str,
    target_name: &Option<String>,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let result = match analyze_target {
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
                    Some(space_name),
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
    base.lifecycle.mark_closed();
    result
}

pub(super) fn execute_migrate(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    action: &MigrateAction,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
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
    base.lifecycle.mark_closed();
    result
}
