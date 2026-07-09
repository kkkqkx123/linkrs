use std::sync::Arc;

use parking_lot::RwLock;

use super::super::super::chunk::{ColumnInfo, DataChunk, Schema};
use crate::core::error::QueryError;
use crate::core::types::edge::EdgeTypeInfo;
use crate::core::types::index::{Index, IndexConfig, IndexType};
use crate::core::types::space::SpaceInfo;
use crate::core::types::tag::TagInfo;
use crate::core::types::user::UserInfo;
use crate::core::{NullType, Value};
use crate::storage::{StorageClient, StorageSchemaOps, StorageAuthOps};
use super::super::StreamingExecutor;

fn make_manage_result(action: &str, name: Option<&str>, status: &str) -> DataChunk {
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
            Value::String(status.to_string()),
        ]],
        schema,
    )
}

fn exec_ddl<F>(storage: &Option<Arc<RwLock<dyn StorageClient>>>, f: F) -> Result<Option<DataChunk>, QueryError>
where
    F: FnOnce(&mut dyn StorageSchemaOps) -> Result<(), QueryError>,
{
    if let Some(lock) = storage {
        let mut writer = lock.write();
        f(&mut *writer).map(|_| Some(make_manage_result("ddl", None, "executed")))
    } else {
        Ok(Some(make_manage_result("ddl", None, "no-storage")))
    }
}

fn exec_auth<F>(storage: &Option<Arc<RwLock<dyn StorageClient>>>, f: F) -> Result<Option<DataChunk>, QueryError>
where
    F: FnOnce(&mut dyn StorageAuthOps) -> Result<(), QueryError>,
{
    if let Some(lock) = storage {
        let mut writer = lock.write();
        f(&mut *writer).map(|_| Some(make_manage_result("auth", None, "executed")))
    } else {
        Ok(Some(make_manage_result("auth", None, "no-storage")))
    }
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
        StreamingExecutor::SpaceManage {
            storage,
            action,
            space_name,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("SpaceManage not opened".to_string()));
            }

            let result = match action.as_str() {
                "create_space" | "create" => {
                    exec_ddl(storage, |s| {
                        let mut info = SpaceInfo::new(space_name.clone().unwrap_or_default());
                        StorageSchemaOps::create_space(s, &mut info).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "drop_space" | "drop" => {
                    exec_ddl(storage, |s| {
                        let name = space_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_space(s, name).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                _ => Ok(Some(make_manage_result(action, space_name.as_deref(), "executed"))),
            };

            *opened = false;
            result
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
        StreamingExecutor::TagManage {
            storage,
            space_name,
            action,
            tag_name,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("TagManage not opened".to_string()));
            }

            let result = match action.as_str() {
                "create_tag" | "create" => {
                    exec_ddl(storage, |s| {
                        let tag_name = tag_name.as_deref().unwrap_or("unnamed");
                        let info = TagInfo::new(tag_name.to_string());
                        StorageSchemaOps::create_tag(s, space_name, &info).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "drop_tag" | "drop" => {
                    exec_ddl(storage, |s| {
                        let name = tag_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_tag(s, space_name, name).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                _ => Ok(Some(make_manage_result(action, tag_name.as_deref(), "executed"))),
            };

            *opened = false;
            result
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
        StreamingExecutor::EdgeManage {
            storage,
            space_name,
            action,
            edge_type,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("EdgeManage not opened".to_string()));
            }

            let result = match action.as_str() {
                "create_edge" | "create" => {
                    exec_ddl(storage, |s| {
                        let et = edge_type.as_deref().unwrap_or("unnamed");
                        let info = EdgeTypeInfo::new(et.to_string());
                        StorageSchemaOps::create_edge_type(s, space_name, &info).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "drop_edge" | "drop" => {
                    exec_ddl(storage, |s| {
                        let name = edge_type.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_edge_type(s, space_name, name).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                _ => Ok(Some(make_manage_result(action, edge_type.as_deref(), "executed"))),
            };

            *opened = false;
            result
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
        StreamingExecutor::IndexManage {
            storage,
            space_name,
            action,
            index_name,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("IndexManage not opened".to_string()));
            }

            let result = match action.as_str() {
                "create_index" | "create" => {
                    exec_ddl(storage, |s| {
                        let idx_name = index_name.as_deref().unwrap_or("unnamed");
                        let info = Index::new(IndexConfig {
                            id: 0,
                            name: idx_name.to_string(),
                            space_id: 0,
                            schema_name: space_name.clone(),
                            fields: vec![],
                            properties: vec![],
                            index_type: IndexType::TagIndex,
                            is_unique: false,
                            partial_condition: None,
                        });
                        StorageSchemaOps::create_tag_index(s, space_name, &info).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "drop_index" | "drop" => {
                    exec_ddl(storage, |s| {
                        let name = index_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_tag_index(s, space_name, name).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                _ => Ok(Some(make_manage_result(action, index_name.as_deref(), "executed"))),
            };

            *opened = false;
            result
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
        StreamingExecutor::UserManage {
            storage,
            action,
            username,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("UserManage not opened".to_string()));
            }

            let result = match action.as_str() {
                "create_user" | "create" => {
                    exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("unknown");
                        let info = UserInfo::new(name.to_string(), "".to_string())
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        StorageAuthOps::create_user(s, &info).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "drop_user" | "drop" => {
                    exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("");
                        StorageAuthOps::drop_user(s, name).map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                _ => Ok(Some(make_manage_result(action, username.as_deref(), "executed"))),
            };

            *opened = false;
            result
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
        StreamingExecutor::FulltextManage {
            action,
            index_name,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("FulltextManage not opened".to_string()));
            }
            *opened = false;
            Ok(Some(make_manage_result(action, index_name.as_deref(), "executed")))
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
        StreamingExecutor::VectorManage {
            action,
            index_name,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("VectorManage not opened".to_string()));
            }
            *opened = false;
            Ok(Some(make_manage_result(action, index_name.as_deref(), "executed")))
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
