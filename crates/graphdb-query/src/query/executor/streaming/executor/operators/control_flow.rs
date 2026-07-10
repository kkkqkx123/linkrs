//! Control flow operation implementations
//!
//! Implements 7 control flow operators:
//! - Loop, Select, PassThrough (control flow)
//! - BeginTransaction, Commit, Rollback (transaction management)
//! - ShowStats (monitoring)

use std::sync::Arc;

use super::super::super::chunk::{ColumnInfo, DataChunk, Schema};
use super::super::StreamingExecutor;
use crate::core::error::QueryError;
use crate::core::Value;

// ============ Loop ============

pub fn open_loop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Loop { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_loop".to_string(),
        )),
    }
}

pub fn next_loop(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Loop { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("Loop not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_loop".to_string(),
        )),
    }
}

pub fn stop_loop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Loop { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_loop".to_string(),
        )),
    }
}

pub fn close_loop(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Loop { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_loop".to_string(),
        )),
    }
}

// ============ Select ============

pub fn open_select(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Select { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_select".to_string(),
        )),
    }
}

pub fn next_select(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Select { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("Select not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_select".to_string(),
        )),
    }
}

pub fn stop_select(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Select { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_select".to_string(),
        )),
    }
}

pub fn close_select(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Select { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_select".to_string(),
        )),
    }
}

// ============ PassThrough ============

pub fn open_passthrough(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PassThrough { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_passthrough".to_string(),
        )),
    }
}

pub fn next_passthrough(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PassThrough { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("PassThrough not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_passthrough".to_string(),
        )),
    }
}

pub fn stop_passthrough(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PassThrough { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_passthrough".to_string(),
        )),
    }
}

pub fn close_passthrough(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PassThrough { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_passthrough".to_string(),
        )),
    }
}

// ============ BeginTransaction ============

pub fn open_begin_transaction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BeginTransaction { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_begin_transaction".to_string(),
        )),
    }
}

pub fn next_begin_transaction(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BeginTransaction { opened, .. } => {
            if !*opened {
                return Err(QueryError::execution(
                    "BeginTransaction not opened".to_string(),
                ));
            }
            *opened = false;
            let schema = Arc::new(Schema::new(vec![ColumnInfo {
                name: "transaction".to_string(),
                data_type: "string".to_string(),
            }]));
            Ok(Some(DataChunk::new(
                vec![vec![Value::String("transaction started".to_string())]],
                schema,
            )))
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_begin_transaction".to_string(),
        )),
    }
}

pub fn stop_begin_transaction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BeginTransaction { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_begin_transaction".to_string(),
        )),
    }
}

pub fn close_begin_transaction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BeginTransaction { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_begin_transaction".to_string(),
        )),
    }
}

// ============ Commit ============

pub fn open_commit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Commit { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_commit".to_string(),
        )),
    }
}

pub fn next_commit(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Commit { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("Commit not opened".to_string()));
            }
            *opened = false;
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            let schema = Arc::new(Schema::new(vec![ColumnInfo {
                name: "transaction".to_string(),
                data_type: "string".to_string(),
            }]));
            Ok(Some(DataChunk::new(
                vec![vec![Value::String("committed".to_string())]],
                schema,
            )))
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_commit".to_string(),
        )),
    }
}

pub fn stop_commit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Commit { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_commit".to_string(),
        )),
    }
}

pub fn close_commit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Commit { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_commit".to_string(),
        )),
    }
}

// ============ Rollback ============

pub fn open_rollback(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Rollback { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_rollback".to_string(),
        )),
    }
}

pub fn next_rollback(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Rollback { opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("Rollback not opened".to_string()));
            }
            *opened = false;
            let schema = Arc::new(Schema::new(vec![ColumnInfo {
                name: "transaction".to_string(),
                data_type: "string".to_string(),
            }]));
            Ok(Some(DataChunk::new(
                vec![vec![Value::String("rolled back".to_string())]],
                schema,
            )))
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_rollback".to_string(),
        )),
    }
}

pub fn stop_rollback(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Rollback { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_rollback".to_string(),
        )),
    }
}

pub fn close_rollback(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Rollback { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_rollback".to_string(),
        )),
    }
}

// ============ ShowStats ============

pub fn open_show_stats(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShowStats { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_show_stats".to_string(),
        )),
    }
}

pub fn next_show_stats(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ShowStats {
            storage, opened, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ShowStats not opened".to_string()));
            }
            *opened = false;

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
                        Value::String("total_vertices".to_string()),
                        Value::BigInt(stats.total_vertices as i64),
                    ],
                    vec![
                        Value::String("total_edges".to_string()),
                        Value::BigInt(stats.total_edges as i64),
                    ],
                    vec![
                        Value::String("total_spaces".to_string()),
                        Value::BigInt(stats.total_spaces as i64),
                    ],
                    vec![
                        Value::String("total_tags".to_string()),
                        Value::BigInt(stats.total_tags as i64),
                    ],
                    vec![
                        Value::String("total_edge_types".to_string()),
                        Value::BigInt(stats.total_edge_types as i64),
                    ],
                    vec![
                        Value::String("total_size_bytes".to_string()),
                        Value::BigInt(stats.total_size_bytes as i64),
                    ],
                    vec![
                        Value::String("data_size_bytes".to_string()),
                        Value::BigInt(stats.data_size_bytes as i64),
                    ],
                    vec![
                        Value::String("index_size_bytes".to_string()),
                        Value::BigInt(stats.index_size_bytes as i64),
                    ],
                ];
                Ok(Some(DataChunk::new(rows, schema)))
            } else {
                let schema = make_single_col_schema("message", "string");
                Ok(Some(DataChunk::new(
                    vec![vec![Value::String("no storage available".to_string())]],
                    schema,
                )))
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_show_stats".to_string(),
        )),
    }
}

fn make_single_col_schema(col_name: &str, col_type: &str) -> Arc<Schema> {
    Arc::new(Schema::new(vec![ColumnInfo {
        name: col_name.to_string(),
        data_type: col_type.to_string(),
    }]))
}

pub fn stop_show_stats(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShowStats { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_show_stats".to_string(),
        )),
    }
}

pub fn close_show_stats(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShowStats { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_show_stats".to_string(),
        )),
    }
}
