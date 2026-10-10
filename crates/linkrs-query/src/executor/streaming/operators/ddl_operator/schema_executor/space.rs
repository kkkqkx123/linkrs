use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::SpaceManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::space::SpaceInfo;
use linkrs_core::{NullType, Value};

use super::common::parse_vid_type_str;

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
    let space_name = match command {
        SpaceManageCommand::Create { space_name, .. }
        | SpaceManageCommand::Drop { space_name }
        | SpaceManageCommand::Desc { space_name }
        | SpaceManageCommand::ShowCreate { space_name }
        | SpaceManageCommand::Switch { space_name }
        | SpaceManageCommand::Alter { space_name }
        | SpaceManageCommand::Clear { space_name }
        | SpaceManageCommand::CommentOn { space_name, .. } => Some(space_name.clone()),
        SpaceManageCommand::Show | SpaceManageCommand::Checkpoint => None,
        SpaceManageCommand::ExportDatabase { .. }
        | SpaceManageCommand::ImportDatabase { .. }
        | SpaceManageCommand::AttachDatabase { .. }
        | SpaceManageCommand::DetachDatabase { .. } => None,
    };
    let result = match command {
        SpaceManageCommand::Create {
            space_name,
            vid_type,
        } => super::super::exec_ddl(storage, |s| {
            let vid_type = parse_vid_type_str(vid_type).map_err(QueryError::execution)?;
            let mut space_info = SpaceInfo::new(space_name.clone()).with_vid_type(vid_type);
            StorageSchemaOps::create_space(s, &mut space_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Drop { .. } => super::super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Alter { .. } => {
            let comment = space_name.as_deref().unwrap_or("");
            super::super::exec_ddl(storage, |s| {
                StorageSchemaOps::alter_space_comment(s, 0, comment.to_string())
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                Ok(())
            })
        }
        SpaceManageCommand::Clear { .. } => super::super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::clear_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Desc { .. } | SpaceManageCommand::ShowCreate { .. } => {
            let reader = super::super::get_reader(storage)?;
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
        SpaceManageCommand::Switch { .. } => {
            let reader = super::super::get_reader(storage)?;
            let name = space_name.as_deref().unwrap_or("");
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
        SpaceManageCommand::CommentOn {
            space_name: _,
            comment,
        } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::alter_space_comment(s, 0, comment.clone())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Checkpoint => super::super::exec_auth(storage, |s| {
            let result = s
                .create_checkpoint()
                .map_err(|e| QueryError::execution(format!("Checkpoint failed: {}", e)))?;
            match result {
                Some(_stats) => Ok(()),
                None => Ok(()),
            }
        }),
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
