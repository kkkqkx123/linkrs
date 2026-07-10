use std::sync::Arc;

use parking_lot::RwLock;

use super::super::super::chunk::{ColumnInfo, DataChunk, Schema};
use super::super::StreamingExecutor;
use crate::core::error::QueryError;
use crate::core::permission::RoleType;
use crate::core::types::edge::EdgeTypeInfo;
use crate::core::types::index::{Index, IndexConfig, IndexType};
use crate::core::types::space::SpaceInfo;
use crate::core::types::tag::TagInfo;
use crate::core::types::user::{PasswordInfo, UserAlterInfo, UserInfo};
use crate::core::{NullType, Value};
use crate::storage::{StorageAuthOps, StorageClient, StorageSchemaOps};

fn make_manage_result(action: &str, name: Option<&str>, status: &str) -> DataChunk {
    let name_val = name
        .map(|n| Value::String(n.to_string()))
        .unwrap_or(Value::Null(NullType::Null));
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "action".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "status".to_string(),
            data_type: "string".to_string(),
        },
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

fn exec_ddl<F>(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    f: F,
) -> Result<Option<DataChunk>, QueryError>
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

fn exec_auth<F>(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    f: F,
) -> Result<Option<DataChunk>, QueryError>
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

fn get_reader(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
) -> Result<parking_lot::RwLockReadGuard<dyn StorageClient>, QueryError> {
    storage
        .as_ref()
        .map(|s| s.read())
        .ok_or_else(|| QueryError::execution("No storage available".to_string()))
}

fn make_single_row(schema: Arc<Schema>, cols: Vec<Value>) -> DataChunk {
    DataChunk::new(vec![cols], schema)
}

fn make_single_col_schema(col_name: &str, col_type: &str) -> Arc<Schema> {
    Arc::new(Schema::new(vec![ColumnInfo {
        name: col_name.to_string(),
        data_type: col_type.to_string(),
    }]))
}

// ============ SpaceManage ============

pub fn open_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_space_manage".to_string(),
        )),
    }
}

pub fn next_space_manage(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::SpaceManage {
            storage,
            action,
            space_name,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_space" | "create" => exec_ddl(storage, |s| {
                    let mut info = SpaceInfo::new(space_name.clone().unwrap_or_default());
                    StorageSchemaOps::create_space(s, &mut info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "drop_space" | "drop" => exec_ddl(storage, |s| {
                    let name = space_name.as_deref().unwrap_or("");
                    StorageSchemaOps::drop_space(s, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "alter_space" | "alter" | "alter_space_comment" => {
                    let comment = space_name.as_deref().unwrap_or("");
                    exec_ddl(storage, |s| {
                        StorageSchemaOps::alter_space_comment(s, 0, comment.to_string())
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    })
                }
                "clear_space" | "clear" => exec_ddl(storage, |s| {
                    let name = space_name.as_deref().unwrap_or("");
                    StorageSchemaOps::clear_space(s, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "desc_space" | "desc" => {
                    let reader = get_reader(storage)?;
                    let name = space_name.as_deref().unwrap_or("");
                    match reader
                        .get_space(name)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                    {
                        Some(info) => {
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "id".to_string(),
                                    data_type: "bigint".to_string(),
                                },
                                ColumnInfo {
                                    name: "vid_type".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "partition_num".to_string(),
                                    data_type: "int".to_string(),
                                },
                                ColumnInfo {
                                    name: "replica_factor".to_string(),
                                    data_type: "int".to_string(),
                                },
                                ColumnInfo {
                                    name: "comment".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "status".to_string(),
                                    data_type: "string".to_string(),
                                },
                            ]));
                            Ok(Some(make_single_row(
                                schema,
                                vec![
                                    Value::String(info.space_name),
                                    Value::BigInt(info.space_id as i64),
                                    Value::String(format!("{:?}", info.vid_type)),
                                    Value::Int(info.partition_num),
                                    Value::Int(info.replica_factor),
                                    info.comment
                                        .clone()
                                        .map(Value::String)
                                        .unwrap_or(Value::Null(NullType::Null)),
                                    Value::String(format!("{:?}", info.status)),
                                ],
                            )))
                        }
                        None => Ok(Some(make_manage_result(
                            "desc_space",
                            Some(name),
                            "not-found",
                        ))),
                    }
                }
                "show_spaces" => {
                    let reader = get_reader(storage)?;
                    let spaces = reader
                        .list_spaces()
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "bigint".to_string(),
                        },
                        ColumnInfo {
                            name: "vid_type".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "status".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows: Vec<Vec<Value>> = spaces
                        .into_iter()
                        .map(|s| {
                            vec![
                                Value::String(s.space_name),
                                Value::BigInt(s.space_id as i64),
                                Value::String(format!("{:?}", s.vid_type)),
                                Value::String(format!("{:?}", s.status)),
                            ]
                        })
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                "switch_space" => Ok(Some(make_manage_result(
                    "switch_space",
                    space_name.as_deref(),
                    "switched",
                ))),
                _ => Err(QueryError::execution(format!(
                    "Unsupported space action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_space_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_space_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_space_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_tag_manage".to_string(),
        )),
    }
}

pub fn next_tag_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TagManage {
            storage,
            space_name,
            action,
            tag_name,
            properties,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_tag" | "create" => exec_ddl(storage, |s| {
                    let tag_name = tag_name.as_deref().unwrap_or("unnamed");
                    let mut info = TagInfo::new(tag_name.to_string());
                    info.properties = properties.clone();
                    StorageSchemaOps::create_tag(s, space_name, &info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "drop_tag" | "drop" => exec_ddl(storage, |s| {
                    let name = tag_name.as_deref().unwrap_or("");
                    StorageSchemaOps::drop_tag(s, space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "alter_tag" | "alter" => exec_ddl(storage, |s| {
                    let name = tag_name.as_deref().unwrap_or("");
                    StorageSchemaOps::alter_tag(s, space_name, name, vec![], vec![])
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "desc_tag" | "desc" => {
                    let reader = get_reader(storage)?;
                    let name = tag_name.as_deref().unwrap_or("");
                    match reader
                        .get_tag(space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                    {
                        Some(tag) => {
                            let props_str: String = tag
                                .properties
                                .iter()
                                .map(|p| format!("{}:{:?}", p.name, p.data_type))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "id".to_string(),
                                    data_type: "bigint".to_string(),
                                },
                                ColumnInfo {
                                    name: "properties".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "comment".to_string(),
                                    data_type: "string".to_string(),
                                },
                            ]));
                            Ok(Some(make_single_row(
                                schema,
                                vec![
                                    Value::String(tag.tag_name),
                                    Value::BigInt(tag.tag_id as i64),
                                    Value::String(props_str),
                                    tag.comment
                                        .clone()
                                        .map(Value::String)
                                        .unwrap_or(Value::Null(NullType::Null)),
                                ],
                            )))
                        }
                        None => Ok(Some(make_manage_result(
                            "desc_tag",
                            Some(name),
                            "not-found",
                        ))),
                    }
                }
                "show_tags" => {
                    let reader = get_reader(storage)?;
                    let tags = reader
                        .list_tags(space_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "bigint".to_string(),
                        },
                        ColumnInfo {
                            name: "properties".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows: Vec<Vec<Value>> = tags
                        .into_iter()
                        .map(|t| {
                            let props_str: String = t
                                .properties
                                .iter()
                                .map(|p| format!("{}:{:?}", p.name, p.data_type))
                                .collect::<Vec<_>>()
                                .join(", ");
                            vec![
                                Value::String(t.tag_name),
                                Value::BigInt(t.tag_id as i64),
                                Value::String(props_str),
                            ]
                        })
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                "show_create_tag" => {
                    let reader = get_reader(storage)?;
                    let name = tag_name.as_deref().unwrap_or("");
                    match reader
                        .get_tag(space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                    {
                        Some(tag) => {
                            let ddl = format!(
                                "CREATE TAG {} ({})",
                                tag.tag_name,
                                tag.properties
                                    .iter()
                                    .map(|p| format!("{} {:?}", p.name, p.data_type))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            let schema = make_single_col_schema("create_tag", "string");
                            Ok(Some(make_single_row(schema, vec![Value::String(ddl)])))
                        }
                        None => Ok(Some(make_manage_result(
                            "show_create_tag",
                            Some(name),
                            "not-found",
                        ))),
                    }
                }
                _ => Err(QueryError::execution(format!(
                    "Unsupported tag action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_tag_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_tag_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_tag_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_edge_manage".to_string(),
        )),
    }
}

pub fn next_edge_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::EdgeManage {
            storage,
            space_name,
            action,
            edge_type,
            properties,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_edge" | "create" => exec_ddl(storage, |s| {
                    let et = edge_type.as_deref().unwrap_or("unnamed");
                    let mut info = EdgeTypeInfo::new(et.to_string());
                    info.properties = properties.clone();
                    StorageSchemaOps::create_edge_type(s, space_name, &info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "drop_edge" | "drop" => exec_ddl(storage, |s| {
                    let name = edge_type.as_deref().unwrap_or("");
                    StorageSchemaOps::drop_edge_type(s, space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "alter_edge" | "alter" => exec_ddl(storage, |s| {
                    let name = edge_type.as_deref().unwrap_or("");
                    StorageSchemaOps::alter_edge_type(s, space_name, name, vec![], vec![])
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "desc_edge" | "desc" => {
                    let reader = get_reader(storage)?;
                    let name = edge_type.as_deref().unwrap_or("");
                    match reader
                        .get_edge_type(space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                    {
                        Some(et) => {
                            let props_str: String = et
                                .properties
                                .iter()
                                .map(|p| format!("{}:{:?}", p.name, p.data_type))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "id".to_string(),
                                    data_type: "bigint".to_string(),
                                },
                                ColumnInfo {
                                    name: "src_tag".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "dst_tag".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "properties".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "comment".to_string(),
                                    data_type: "string".to_string(),
                                },
                            ]));
                            Ok(Some(make_single_row(
                                schema,
                                vec![
                                    Value::String(et.edge_type_name),
                                    Value::BigInt(et.edge_type_id as i64),
                                    Value::String(et.src_tag_name),
                                    Value::String(et.dst_tag_name),
                                    Value::String(props_str),
                                    et.comment
                                        .clone()
                                        .map(Value::String)
                                        .unwrap_or(Value::Null(NullType::Null)),
                                ],
                            )))
                        }
                        None => Ok(Some(make_manage_result(
                            "desc_edge",
                            Some(name),
                            "not-found",
                        ))),
                    }
                }
                "show_edges" => {
                    let reader = get_reader(storage)?;
                    let edges = reader
                        .list_edge_types(space_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "bigint".to_string(),
                        },
                        ColumnInfo {
                            name: "src_tag".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "dst_tag".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows: Vec<Vec<Value>> = edges
                        .into_iter()
                        .map(|e| {
                            vec![
                                Value::String(e.edge_type_name),
                                Value::BigInt(e.edge_type_id as i64),
                                Value::String(e.src_tag_name),
                                Value::String(e.dst_tag_name),
                            ]
                        })
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                _ => Err(QueryError::execution(format!(
                    "Unsupported edge action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_edge_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_edge_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_edge_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_index_manage".to_string(),
        )),
    }
}

pub fn next_index_manage(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_index" | "create" | "create_tag_index" => exec_ddl(storage, |s| {
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
                    StorageSchemaOps::create_tag_index(s, space_name, &info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "create_edge_index" => Err(QueryError::execution(
                    "Edge index creation is not exposed by StorageSchemaOps".to_string(),
                )),
                "drop_index" | "drop" | "drop_tag_index" => exec_ddl(storage, |s| {
                    let name = index_name.as_deref().unwrap_or("");
                    StorageSchemaOps::drop_tag_index(s, space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "drop_edge_index" => Err(QueryError::execution(
                    "Edge index deletion is not exposed by StorageSchemaOps".to_string(),
                )),
                "desc_index" | "desc" | "describe_index" => {
                    let reader = get_reader(storage)?;
                    let name = index_name.as_deref().unwrap_or("");
                    match reader
                        .get_tag_index(space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                    {
                        Some(idx) => {
                            let fields_str: String = idx
                                .fields
                                .iter()
                                .map(|f| f.name.clone())
                                .collect::<Vec<_>>()
                                .join(", ");
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "index_type".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "fields".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "status".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "unique".to_string(),
                                    data_type: "bool".to_string(),
                                },
                            ]));
                            Ok(Some(make_single_row(
                                schema,
                                vec![
                                    Value::String(idx.name),
                                    Value::String(format!("{:?}", idx.index_type)),
                                    Value::String(fields_str),
                                    Value::String(format!("{:?}", idx.status)),
                                    Value::Bool(idx.is_unique),
                                ],
                            )))
                        }
                        None => Ok(Some(make_manage_result(
                            "desc_index",
                            Some(name),
                            "not-found",
                        ))),
                    }
                }
                "show_indexes" | "show" | "show_tag_indexes" => {
                    let reader = get_reader(storage)?;
                    let indexes = reader
                        .list_tag_indexes(space_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "index_type".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "fields".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "status".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows: Vec<Vec<Value>> = indexes
                        .into_iter()
                        .map(|idx| {
                            let fields_str: String = idx
                                .fields
                                .iter()
                                .map(|f| f.name.clone())
                                .collect::<Vec<_>>()
                                .join(", ");
                            vec![
                                Value::String(idx.name),
                                Value::String(format!("{:?}", idx.index_type)),
                                Value::String(fields_str),
                                Value::String(format!("{:?}", idx.status)),
                            ]
                        })
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                "rebuild_index" | "rebuild" => exec_ddl(storage, |s| {
                    let name = index_name.as_deref().unwrap_or("");
                    StorageSchemaOps::rebuild_tag_index(s, space_name, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                _ => Err(QueryError::execution(format!(
                    "Unsupported index action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_index_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_index_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_index_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_user_manage".to_string(),
        )),
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
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_user" | "create" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("unknown");
                    let info = UserInfo::new(name.to_string(), "".to_string())
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    StorageAuthOps::create_user(s, &info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "drop_user" | "drop" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("");
                    StorageAuthOps::drop_user(s, name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "alter_user" | "alter" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("");
                    let alter_info = UserAlterInfo::new(name.to_string());
                    StorageAuthOps::alter_user(s, &alter_info)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "describe_user" | "describe" => {
                    let reader = get_reader(storage)?;
                    let name = username.as_deref().unwrap_or("");
                    let exists = reader.user_exists(name);
                    if exists {
                        let schema = make_single_col_schema("user", "string");
                        Ok(Some(make_single_row(
                            schema,
                            vec![Value::String(format!("User '{}' exists", name))],
                        )))
                    } else {
                        Ok(Some(make_manage_result(
                            "describe_user",
                            Some(name),
                            "not-found",
                        )))
                    }
                }
                "change_password" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("");
                    let pw = PasswordInfo {
                        username: Some(name.to_string()),
                        old_password: String::new(),
                        new_password: String::new(),
                    };
                    StorageAuthOps::change_password(s, &pw)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "grant_role" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("");
                    StorageAuthOps::grant_role(s, name, 0, RoleType::User)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                "revoke_role" => exec_auth(storage, |s| {
                    let name = username.as_deref().unwrap_or("");
                    StorageAuthOps::revoke_role(s, name, 0)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    Ok(())
                }),
                _ => Err(QueryError::execution(format!(
                    "Unsupported user action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_user_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_user_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_user_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_fulltext_manage".to_string(),
        )),
    }
}

pub fn next_fulltext_manage(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextManage {
            storage: _storage,
            space_name: _space_name,
            space_id: _space_id,
            action: _action,
            index_name: _index_name,
            tag_name: _tag_name,
            field_name: _field_name,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }
            *opened = false;

            #[cfg(feature = "fulltext-search")]
            {
                return next_fulltext_manage_impl(executor);
            }

            #[cfg(not(feature = "fulltext-search"))]
            Ok(Some(make_manage_result(
                _action,
                _index_name.as_deref(),
                "fulltext-search feature disabled",
            )))
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_fulltext_manage".to_string(),
        )),
    }
}

#[cfg(feature = "fulltext-search")]
fn next_fulltext_manage_impl(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextManage {
            action,
            index_name,
            tag_name,
            field_name,
            space_id,
            fulltext_manager,
            opened,
            ..
        } => {
            *opened = false;
            let result = match action.as_str() {
                "create_fulltext_index" | "create" => {
                    if let Some(manager) = fulltext_manager {
                        let sid = *space_id;
                        let tn = tag_name.as_deref().unwrap_or("");
                        let fn_ = field_name.as_deref().unwrap_or("");
                        futures::executor::block_on(manager.create_index(sid, tn, fn_, None))
                            .map_err(|e| {
                                QueryError::execution(format!("Fulltext create failed: {}", e))
                            })?;
                        Some(make_manage_result(
                            "create_fulltext_index",
                            index_name.as_deref(),
                            "created",
                        ))
                    } else {
                        Some(make_manage_result(
                            "create_fulltext_index",
                            index_name.as_deref(),
                            "no-manager",
                        ))
                    }
                }
                "drop_fulltext_index" | "drop" => {
                    if let Some(manager) = fulltext_manager {
                        let sid = *space_id;
                        let tn = tag_name.as_deref().unwrap_or("");
                        let fn_ = field_name.as_deref().unwrap_or("");
                        futures::executor::block_on(manager.drop_index(sid, tn, fn_)).map_err(
                            |e| QueryError::execution(format!("Fulltext drop failed: {}", e)),
                        )?;
                        Some(make_manage_result(
                            "drop_fulltext_index",
                            index_name.as_deref(),
                            "dropped",
                        ))
                    } else {
                        Some(make_manage_result(
                            "drop_fulltext_index",
                            index_name.as_deref(),
                            "no-manager",
                        ))
                    }
                }
                "describe_fulltext_index" | "desc" => {
                    if let Some(manager) = fulltext_manager {
                        let sid = *space_id;
                        let tn = tag_name.as_deref().unwrap_or("");
                        let fn_ = field_name.as_deref().unwrap_or("");
                        if let Some(meta) = manager.get_metadata(sid, tn, fn_) {
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "index_id".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "tag_name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "field_name".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "status".to_string(),
                                    data_type: "string".to_string(),
                                },
                            ]));
                            Some(make_single_row(
                                schema,
                                vec![
                                    Value::String(meta.index_name),
                                    Value::String(meta.tag_name),
                                    Value::String(meta.field_name),
                                    Value::String(format!("{:?}", meta.status)),
                                ],
                            ))
                        } else {
                            Some(make_manage_result(
                                "describe_fulltext_index",
                                index_name.as_deref(),
                                "not-found",
                            ))
                        }
                    } else {
                        Some(make_manage_result(
                            "describe_fulltext_index",
                            index_name.as_deref(),
                            "no-manager",
                        ))
                    }
                }
                "show_fulltext_indexes" | "show" => {
                    if let Some(manager) = fulltext_manager {
                        let indexes = manager.list_indexes();
                        let schema = Arc::new(Schema::new(vec![
                            ColumnInfo {
                                name: "index_id".to_string(),
                                data_type: "string".to_string(),
                            },
                            ColumnInfo {
                                name: "space_id".to_string(),
                                data_type: "bigint".to_string(),
                            },
                            ColumnInfo {
                                name: "tag_name".to_string(),
                                data_type: "string".to_string(),
                            },
                            ColumnInfo {
                                name: "field_name".to_string(),
                                data_type: "string".to_string(),
                            },
                            ColumnInfo {
                                name: "status".to_string(),
                                data_type: "string".to_string(),
                            },
                        ]));
                        let rows: Vec<Vec<Value>> = indexes
                            .into_iter()
                            .map(|m| {
                                vec![
                                    Value::String(m.index_name),
                                    Value::BigInt(m.space_id as i64),
                                    Value::String(m.tag_name),
                                    Value::String(m.field_name),
                                    Value::String(format!("{:?}", m.status)),
                                ]
                            })
                            .collect();
                        Some(DataChunk::new(rows, schema))
                    } else {
                        Some(make_manage_result(
                            "show_fulltext_indexes",
                            None,
                            "no-manager",
                        ))
                    }
                }
                _ => {
                    return Err(QueryError::execution(format!(
                        "Unsupported fulltext action: {}",
                        action
                    )));
                }
            };
            Ok(result)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_fulltext_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_fulltext_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_fulltext_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in open_vector_manage".to_string(),
        )),
    }
}

pub fn next_vector_manage(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorManage {
            action,
            index_name,
            tag_name,
            field_name,
            space_id,
            storage,
            space_name,
            opened,
            #[cfg(feature = "qdrant")]
            vector_coordinator,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

            let result = match action.as_str() {
                "create_vector_index" | "create" => {
                    #[cfg(feature = "qdrant")]
                    {
                        if let Some(coordinator) = vector_coordinator {
                            let tn = tag_name.as_deref().unwrap_or("default_tag");
                            let fn_ = field_name.as_deref().unwrap_or("default_field");
                            let result =
                                futures::executor::block_on(coordinator.create_vector_index(
                                    *space_id,
                                    tn,
                                    fn_,
                                    128,
                                    vector_client::DistanceMetric::Cosine,
                                ))
                                .map_err(|e| {
                                    QueryError::execution(format!("Vector create failed: {}", e))
                                });
                            match result {
                                Ok(_) => Ok(Some(make_manage_result(
                                    "create_vector_index",
                                    index_name.as_deref(),
                                    "created",
                                ))),
                                Err(e) => Err(e),
                            }
                        } else {
                            Ok(Some(make_manage_result(
                                "create_vector_index",
                                index_name.as_deref(),
                                "no-coordinator",
                            )))
                        }
                    }
                    #[cfg(not(feature = "qdrant"))]
                    {
                        let _ = (storage, space_name);
                        Ok(Some(make_manage_result(
                            "create_vector_index",
                            index_name.as_deref(),
                            "qdrant feature disabled",
                        )))
                    }
                }
                "drop_vector_index" | "drop" => {
                    #[cfg(feature = "qdrant")]
                    {
                        if let Some(coordinator) = vector_coordinator {
                            let tn = tag_name.as_deref().unwrap_or("default_tag");
                            let fn_ = field_name.as_deref().unwrap_or("default_field");
                            let result = futures::executor::block_on(
                                coordinator.drop_vector_index(*space_id, tn, fn_),
                            )
                            .map_err(|e| {
                                QueryError::execution(format!("Vector drop failed: {}", e))
                            });
                            match result {
                                Ok(_) => Ok(Some(make_manage_result(
                                    "drop_vector_index",
                                    index_name.as_deref(),
                                    "dropped",
                                ))),
                                Err(e) => Err(e),
                            }
                        } else {
                            Ok(Some(make_manage_result(
                                "drop_vector_index",
                                index_name.as_deref(),
                                "no-coordinator",
                            )))
                        }
                    }
                    #[cfg(not(feature = "qdrant"))]
                    {
                        let _ = (storage, space_name);
                        Ok(Some(make_manage_result(
                            "drop_vector_index",
                            index_name.as_deref(),
                            "qdrant feature disabled",
                        )))
                    }
                }
                _ => Err(QueryError::execution(format!(
                    "Unsupported vector action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_vector_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in stop_vector_manage".to_string(),
        )),
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
        _ => Err(QueryError::execution(
            "Type mismatch in close_vector_manage".to_string(),
        )),
    }
}

// ============ Analyze ============

pub fn open_analyze(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Analyze { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_analyze".to_string(),
        )),
    }
}

pub fn next_analyze(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Analyze {
            storage,
            space_name,
            analyze_target,
            target_name,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

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
                        Ok(Some(make_single_row(
                            schema,
                            vec![
                                Value::String(format!("space:{}", space_name)),
                                Value::String(format!("{:?}", stats)),
                            ],
                        )))
                    } else {
                        Ok(Some(make_manage_result(
                            "analyze",
                            Some(space_name),
                            "no-storage",
                        )))
                    }
                }
                "tag" | "edge" => {
                    let name = target_name.as_deref().unwrap_or("");
                    Ok(Some(make_manage_result("analyze", Some(name), "executed")))
                }
                _ => Err(QueryError::execution(format!(
                    "Unsupported analyze target: {}",
                    analyze_target
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_analyze".to_string(),
        )),
    }
}

pub fn stop_analyze(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Analyze { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_analyze".to_string(),
        )),
    }
}

pub fn close_analyze(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Analyze { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_analyze".to_string(),
        )),
    }
}

// ============ Migrate ============

pub fn open_migrate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Migrate { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_migrate".to_string(),
        )),
    }
}

pub fn next_migrate(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Migrate {
            storage,
            space_name,
            action,
            migration_data,
            opened,
            ..
        } => {
            if !*opened {
                return Ok(None);
            }

            let result = match action.as_str() {
                "migrate_space" | "migrate" | "migrate_vertex" | "migrate_edge" => {
                    if let Some(lock) = storage {
                        let writer = lock.write();
                        let result = writer
                            .save_to_disk()
                            .map_err(|e| QueryError::execution(format!("Migrate failed: {}", e)));
                        match result {
                            Ok(_) => Ok(Some(make_manage_result(
                                "migrate",
                                Some(space_name),
                                "saved",
                            ))),
                            Err(e) => Err(e),
                        }
                    } else {
                        Ok(Some(make_manage_result(
                            "migrate",
                            Some(space_name),
                            "no-storage",
                        )))
                    }
                }
                _ => Err(QueryError::execution(format!(
                    "Unsupported migrate action: {}",
                    action
                ))),
            };

            *opened = false;
            result
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_migrate".to_string(),
        )),
    }
}

pub fn stop_migrate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Migrate { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_migrate".to_string(),
        )),
    }
}

pub fn close_migrate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Migrate { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_migrate".to_string(),
        )),
    }
}
