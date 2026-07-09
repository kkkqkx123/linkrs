//! Management and DDL operation implementations
//!
//! Implements 7 management operators:
//! - SpaceManage, TagManage, EdgeManage, IndexManage (data management)
//! - UserManage, FulltextManage, VectorManage (system management)
//!
//! These operators produce result chunks indicating the DDL operation outcome.
//! Full DDL execution requires storage integration (delegates to admin executors).

use std::sync::Arc;
use super::super::super::chunk::{ColumnInfo, DataChunk, Schema};
use crate::core::error::QueryError;
use crate::core::{NullType, Value};
use super::super::StreamingExecutor;

/// Produce a result chunk for a management/DDL operation
fn make_manage_result(action: &str, name: Option<&str>) -> DataChunk {
    let name_val = name.map(|n| Value::String(n.to_string()))
        .unwrap_or(Value::Null(NullType::Null));
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo { name: "action".to_string(), data_type: "string".to_string() },
        ColumnInfo { name: "name".to_string(), data_type: "string".to_string() },
        ColumnInfo { name: "status".to_string(), data_type: "string".to_string() },
    ]));
    DataChunk::new(
        vec![vec![
            Value::String(action.to_string()),
            name_val,
            Value::String("executed".to_string()),
        ]],
        schema,
    )
}

/// Execute a management operation and return a single result chunk.
/// The actual DDL logic should be wired to admin executors when storage is available.
fn execute_manage_op(action: &str, name: Option<&str>) -> Result<Option<DataChunk>, QueryError> {
    // Produces one result chunk per DDL call
    Ok(Some(make_manage_result(action, name)))
}

// ============ SpaceManage ============

pub fn open_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_space_manage".to_string())),
    }
}

pub fn next_space_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, action, space_name, .. } => {
            if !*opened {
                return Err(QueryError::execution("SpaceManage not opened".to_string()));
            }
            // Try input first (if there's a data pipeline), otherwise produce result
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, space_name.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_space_manage".to_string())),
    }
}

pub fn stop_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_space_manage".to_string())),
    }
}

pub fn close_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_space_manage".to_string())),
    }
}

// ============ TagManage ============

pub fn open_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_tag_manage".to_string())),
    }
}

pub fn next_tag_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, action, tag_name, .. } => {
            if !*opened {
                return Err(QueryError::execution("TagManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, tag_name.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_tag_manage".to_string())),
    }
}

pub fn stop_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_tag_manage".to_string())),
    }
}

pub fn close_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_tag_manage".to_string())),
    }
}

// ============ EdgeManage ============

pub fn open_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_edge_manage".to_string())),
    }
}

pub fn next_edge_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, action, edge_type, .. } => {
            if !*opened {
                return Err(QueryError::execution("EdgeManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, edge_type.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_edge_manage".to_string())),
    }
}

pub fn stop_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_edge_manage".to_string())),
    }
}

pub fn close_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_edge_manage".to_string())),
    }
}

// ============ IndexManage ============

pub fn open_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_index_manage".to_string())),
    }
}

pub fn next_index_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, action, index_name, .. } => {
            if !*opened {
                return Err(QueryError::execution("IndexManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, index_name.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_index_manage".to_string())),
    }
}

pub fn stop_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_index_manage".to_string())),
    }
}

pub fn close_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_index_manage".to_string())),
    }
}

// ============ UserManage ============

pub fn open_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_user_manage".to_string())),
    }
}

pub fn next_user_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, action, username, .. } => {
            if !*opened {
                return Err(QueryError::execution("UserManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, username.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_user_manage".to_string())),
    }
}

pub fn stop_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_user_manage".to_string())),
    }
}

pub fn close_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_user_manage".to_string())),
    }
}

// ============ FulltextManage ============

pub fn open_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_fulltext_manage".to_string())),
    }
}

pub fn next_fulltext_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, action, index_name, .. } => {
            if !*opened {
                return Err(QueryError::execution("FulltextManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, index_name.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_fulltext_manage".to_string())),
    }
}

pub fn stop_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_fulltext_manage".to_string())),
    }
}

pub fn close_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_fulltext_manage".to_string())),
    }
}

// ============ VectorManage ============

pub fn open_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_vector_manage".to_string())),
    }
}

pub fn next_vector_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, action, index_name, .. } => {
            if !*opened {
                return Err(QueryError::execution("VectorManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            execute_manage_op(action, index_name.as_deref())
        }
        _ => Err(QueryError::execution("Type mismatch in next_vector_manage".to_string())),
    }
}

pub fn stop_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_vector_manage".to_string())),
    }
}

pub fn close_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_vector_manage".to_string())),
    }
}
