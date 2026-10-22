use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::SpaceManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::space::SpaceInfo;
use linkrs_core::{NullType, Value};

use super::common::{parse_vid_type_str, render_create_space};

pub(in crate::executor::streaming::operators) fn execute_space_manage(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::SpaceManage {
        storage,
        command,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match command {
        SpaceManageCommand::Create {
            space_name,
            vid_type,
            comment,
        } => super::super::exec_ddl(storage, |s| {
            let vid_type = parse_vid_type_str(vid_type).map_err(QueryError::execution)?;
            let space_info = SpaceInfo::new(space_name.clone())
                .with_vid_type(vid_type)
                .with_comment(comment.clone());
            let mut space_info = space_info;
            StorageSchemaOps::create_space(s, &mut space_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Drop {
            space_name,
            if_exists,
        } => {
            let name = space_name.as_str();
            let Some(lock) = storage else {
                return Ok(Some(super::super::make_manage_result(
                    "drop_space",
                    None,
                    "no-storage",
                )));
            };
            let mut writer = lock.write();
            let dropped = StorageSchemaOps::drop_space(&mut *writer, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            if !dropped && !*if_exists {
                return Err(QueryError::execution(format!(
                    "Space '{}' does not exist",
                    name
                )));
            }
            Ok(Some(super::super::make_manage_result(
                "drop_space",
                Some(name),
                if dropped { "dropped" } else { "not_exists" },
            )))
        }
        SpaceManageCommand::Clear { space_name } => {
            let name = space_name.as_str();
            let Some(lock) = storage else {
                return Err(QueryError::execution(
                    "CLEAR SPACE cannot execute: no storage available".to_string(),
                ));
            };
            // The clear drops every tag, edge type and row of the space, so
            // the counts are read before the wipe and reported afterwards.
            let (cleared_tags, cleared_edge_types, deleted_vertices, deleted_edges) = {
                let reader = lock.read();
                if !reader.space_exists(name) {
                    return Err(QueryError::execution(format!(
                        "Space '{}' does not exist",
                        name
                    )));
                }
                let tags = reader
                    .list_tags(name)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                let edge_types = reader
                    .list_edge_types(name)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                let mut deleted_vertices = 0u64;
                for tag in &tags {
                    deleted_vertices += reader
                        .count_vertices_by_tag(name, &tag.tag_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                let mut deleted_edges = 0u64;
                for edge_type in &edge_types {
                    deleted_edges += reader
                        .count_edges_by_type(name, &edge_type.edge_type_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                (
                    tags.len(),
                    edge_types.len(),
                    deleted_vertices,
                    deleted_edges,
                )
            };
            let cleared = {
                let mut writer = lock.write();
                StorageSchemaOps::clear_space(&mut *writer, name)
                    .map_err(|e| QueryError::execution(e.to_string()))?
            };
            if !cleared {
                return Err(QueryError::execution(format!(
                    "Space '{}' does not exist",
                    name
                )));
            }
            let schema = Arc::new(Schema::new(vec![
                ColumnInfo {
                    name: "space_name".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "cleared_tags".to_string(),
                    data_type: "bigint".to_string(),
                },
                ColumnInfo {
                    name: "cleared_edge_types".to_string(),
                    data_type: "bigint".to_string(),
                },
                ColumnInfo {
                    name: "deleted_vertices".to_string(),
                    data_type: "bigint".to_string(),
                },
                ColumnInfo {
                    name: "deleted_edges".to_string(),
                    data_type: "bigint".to_string(),
                },
            ]));
            Ok(Some(super::super::make_single_row(
                schema,
                vec![
                    Value::string(name),
                    Value::BigInt(cleared_tags as i64),
                    Value::BigInt(cleared_edge_types as i64),
                    Value::BigInt(deleted_vertices as i64),
                    Value::BigInt(deleted_edges as i64),
                ],
            )))
        }
        SpaceManageCommand::Desc { space_name } => {
            let reader = super::super::get_reader(storage)?;
            let name = space_name.as_str();
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
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![
                            Value::string(info.space_name),
                            Value::BigInt(info.space_id as i64),
                            Value::string(format!("{:?}", info.vid_type)),
                            Value::Int(info.partition_num),
                            Value::Int(info.replica_factor),
                            info.comment
                                .clone()
                                .map(Value::string)
                                .unwrap_or(Value::Null(NullType::Null)),
                            Value::string(format!("{:?}", info.status)),
                        ],
                    )))
                }
                None => Ok(Some(super::super::make_manage_result(
                    "desc_space",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        SpaceManageCommand::ShowCreate { space_name } => {
            let reader = super::super::get_reader(storage)?;
            let name = space_name.as_str();
            match reader
                .get_space(name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(info) => {
                    let ddl = render_create_space(
                        &info.space_name,
                        &info.vid_type,
                        info.comment.as_deref(),
                    );
                    let schema = super::super::make_single_col_schema("create_statement", "string");
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![Value::string(ddl)],
                    )))
                }
                None => Ok(Some(super::super::make_manage_result(
                    "show_create_space",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        SpaceManageCommand::Switch { space_name } => {
            let reader = super::super::get_reader(storage)?;
            let name = space_name.as_str();
            match reader
                .get_space(name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(info) => {
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "space_name".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "space_id".to_string(),
                            data_type: "bigint".to_string(),
                        },
                        ColumnInfo {
                            name: "vid_type".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![
                            Value::string(info.space_name),
                            Value::BigInt(info.space_id as i64),
                            Value::string(format!("{:?}", info.vid_type)),
                        ],
                    )))
                }
                None => Err(QueryError::execution(format!("Space not found: {}", name))),
            }
        }
        SpaceManageCommand::Show => {
            let reader = super::super::get_reader(storage)?;
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
                    name: "partition_num".to_string(),
                    data_type: "int".to_string(),
                },
                ColumnInfo {
                    name: "replica_factor".to_string(),
                    data_type: "int".to_string(),
                },
            ]));
            let rows: Vec<Vec<Value>> = spaces
                .into_iter()
                .map(|info| {
                    vec![
                        Value::string(info.space_name),
                        Value::BigInt(info.space_id as i64),
                        Value::string(format!("{:?}", info.vid_type)),
                        Value::Int(info.partition_num),
                        Value::Int(info.replica_factor),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        SpaceManageCommand::PurgeExpired { space_name, target } => {
            use crate::parser::ast::stmt::PurgeTarget;
            let name = space_name.as_str();
            let Some(lock) = storage else {
                return Ok(Some(super::super::make_manage_result(
                    "purge_expired",
                    None,
                    "no-storage",
                )));
            };
            // Space must exist: a purge never touches an unnamed space.
            if !lock.read().space_exists(name) {
                return Err(QueryError::execution(format!(
                    "Space '{}' does not exist",
                    name
                )));
            }
            // A targeted purge must name an existing schema; a schema with no
            // TTL still yields a zero-row entry (handled by storage).
            if let Some(target) = target.as_ref() {
                let reader = lock.read();
                let exists = match target {
                    PurgeTarget::Tag(t) => reader.get_tag(name, t)?.is_some(),
                    PurgeTarget::Edge(e) => reader.get_edge_type(name, e)?.is_some(),
                };
                if !exists {
                    return Err(QueryError::execution(format!(
                        "PURGE EXPIRED target schema not found in space '{}'",
                        name
                    )));
                }
            }
            let storage_target = target.as_ref().map(|target| match target {
                PurgeTarget::Tag(t) => crate::storage::PurgeTarget::Tag(t.as_str()),
                PurgeTarget::Edge(e) => crate::storage::PurgeTarget::Edge(e.as_str()),
            });
            let entries = lock
                .write()
                .purge_expired(name, storage_target)
                .map_err(|e| QueryError::execution(format!("PURGE EXPIRED failed: {}", e)))?;
            let schema = Arc::new(Schema::new(vec![
                ColumnInfo {
                    name: "space_name".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "schema_name".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "schema_type".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "removed_rows".to_string(),
                    data_type: "bigint".to_string(),
                },
            ]));
            let rows: Vec<Vec<Value>> = entries
                .into_iter()
                .map(|entry| {
                    vec![
                        Value::string(name),
                        Value::string(entry.schema_name),
                        Value::string(if entry.is_edge { "EDGE" } else { "TAG" }),
                        Value::BigInt(entry.removed as i64),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        SpaceManageCommand::Checkpoint => {
            let Some(lock) = storage else {
                return Ok(Some(super::super::make_manage_result(
                    "auth",
                    Some(""),
                    "no-storage",
                )));
            };
            let writer = lock.write();
            let outcome = writer
                .create_checkpoint()
                .map_err(|e| QueryError::execution(format!("Checkpoint failed: {}", e)))?;
            // A present storage without persistence returns no snapshot;
            // report it as a no-op instead of masking it as executed.
            let status = if outcome.is_some() {
                "executed"
            } else {
                "no-storage"
            };
            Ok(Some(super::super::make_manage_result(
                "auth",
                Some(""),
                status,
            )))
        }
        SpaceManageCommand::ExportDatabase { path } => {
            super::super::exec_auth(storage, |s| {
                let export_path = std::path::Path::new(path);
                // Export all spaces or a specific one if path encodes a space name
                let reader: &dyn crate::storage::StorageReader = s;
                let spaces = reader
                    .list_spaces()
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                for space_info in &spaces {
                    s.export_space(&space_info.space_name, export_path)
                        .map_err(|e| {
                            QueryError::execution(format!(
                                "Export failed for space '{}': {}",
                                space_info.space_name, e
                            ))
                        })?;
                }
                Ok(())
            })
        }
        SpaceManageCommand::ImportDatabase { path } => {
            super::super::exec_auth(storage, |s| {
                let import_path = std::path::Path::new(path);
                // Import all spaces found in the import directory
                let entries = std::fs::read_dir(import_path).map_err(|e| {
                    QueryError::execution(format!(
                        "Failed to read import directory '{}': {e}",
                        path
                    ))
                })?;
                for entry in entries {
                    let entry = entry.map_err(|e| {
                        QueryError::execution(format!("Failed to read dir entry: {e}"))
                    })?;
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        let space_name = entry_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if !space_name.is_empty() {
                            s.import_space(space_name, import_path).map_err(|e| {
                                QueryError::execution(format!(
                                    "Import failed for space '{}': {}",
                                    space_name, e
                                ))
                            })?;
                        }
                    }
                }
                Ok(())
            })
        }
        SpaceManageCommand::AttachDatabase {
            alias,
            path,
            db_type,
        } => {
            let info = crate::attached::AttachedDatabase::new(
                alias.clone(),
                path.clone(),
                db_type.clone(),
            );
            match crate::attached::attach_database(info) {
                Ok(()) => Ok(Some(super::super::make_manage_result(
                    "attach",
                    Some(alias.as_str()),
                    "attached",
                ))),
                Err(e) => Err(QueryError::execution(e)),
            }
        }
        SpaceManageCommand::DetachDatabase { alias } => {
            match crate::attached::detach_database(alias) {
                Ok(_) => Ok(Some(super::super::make_manage_result(
                    "detach",
                    Some(alias.as_str()),
                    "detached",
                ))),
                Err(e) => Err(QueryError::execution(e)),
            }
        }
    };
    result
}
