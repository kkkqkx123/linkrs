use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::EdgeManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::edge::EdgeTypeInfo;
use linkrs_core::Value;

use super::common::endpoint_rows;

pub(in crate::executor::streaming::operators) fn execute_edge_manage(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::EdgeManage {
        storage,
        space_name,
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
    let edge_type = match command {
        EdgeManageCommand::Create { edge_name, .. }
        | EdgeManageCommand::Alter { edge_name, .. }
        | EdgeManageCommand::Rename {
            old_name: edge_name,
            ..
        }
        | EdgeManageCommand::UpdateEndpoints { edge_name, .. }
        | EdgeManageCommand::Desc { edge_name }
        | EdgeManageCommand::Drop { edge_name, .. }
        | EdgeManageCommand::ShowCreate { edge_name } => Some(edge_name.clone()),
        EdgeManageCommand::Show => None,
    };
    let result = match command {
        EdgeManageCommand::Create {
            edge_name,
            properties,
            src_tag_name,
            dst_tag_name,
            if_not_exists,
        } => super::super::exec_ddl(storage, |s| {
            let mut info = EdgeTypeInfo::new(edge_name.clone());
            info.properties = properties.clone();
            info.src_tag_name = src_tag_name.clone().unwrap_or_default();
            info.dst_tag_name = dst_tag_name.clone().unwrap_or_default();
            match StorageSchemaOps::create_edge_type(s, space_name, &info) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_not_exists
                        && e.kind()
                            == linkrs_core::error::storage::StorageErrorKind::LabelAlreadyExists =>
                {
                    Ok(())
                }
                Err(e) => Err(QueryError::execution(e.to_string())),
            }
        }),
        EdgeManageCommand::Drop {
            edge_name,
            if_exists,
        } => super::super::exec_ddl(storage, |s| {
            match StorageSchemaOps::drop_edge_type(s, space_name, edge_name) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_exists
                        && (e.kind()
                            == linkrs_core::error::storage::StorageErrorKind::LabelNotFound
                            || e.kind()
                                == linkrs_core::error::storage::StorageErrorKind::NotFound) =>
                {
                    Ok(())
                }
                Err(e) => Err(QueryError::execution(e.to_string())),
            }
        }),
        EdgeManageCommand::Alter {
            edge_name,
            additions,
            deletions,
        } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::alter_edge_type(
                s,
                space_name,
                edge_name,
                additions.clone(),
                deletions.clone(),
            )
            .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        EdgeManageCommand::Rename { old_name, new_name } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::rename_edge_type(s, space_name, old_name, new_name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        EdgeManageCommand::UpdateEndpoints {
            edge_name,
            src_tag_name,
            dst_tag_name,
            clear_constraint,
        } => super::super::exec_ddl(storage, |s| {
            let (src, dst) = if *clear_constraint {
                (String::new(), String::new())
            } else {
                (src_tag_name.clone(), dst_tag_name.clone())
            };
            let updated =
                StorageSchemaOps::update_edge_endpoints(s, space_name, edge_name, &src, &dst)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
            if !updated {
                return Err(QueryError::execution(format!(
                    "Edge type '{edge_name}' not found"
                )));
            }
            Ok(())
        }),
        EdgeManageCommand::Desc { .. } | EdgeManageCommand::ShowCreate { .. } => {
            let reader = super::super::get_reader(storage)?;
            let name = edge_type.as_deref().unwrap_or("");
            match reader
                .get_edge_type(space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(et) => {
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "Field".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "Type".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "Nullable".to_string(),
                            data_type: "bool".to_string(),
                        },
                        ColumnInfo {
                            name: "Default".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "Comment".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows: Vec<Vec<Value>> = endpoint_rows(&et)
                        .into_iter()
                        .chain(et.properties.iter().map(|p| {
                            vec![
                                Value::string(&p.name),
                                Value::string(p.data_type.to_string()),
                                Value::Bool(p.nullable),
                                p.default
                                    .as_ref()
                                    .map(|v| Value::string(format!("{}", v)))
                                    .unwrap_or_else(|| Value::string("")),
                                p.comment
                                    .as_ref()
                                    .map(|c| Value::string(c.clone()))
                                    .unwrap_or_else(|| Value::string("")),
                            ]
                        }))
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                None => {
                    let schema = super::super::make_single_col_schema("Field", "string");
                    Ok(Some(DataChunk::new(vec![], schema)))
                }
            }
        }
        EdgeManageCommand::Show => {
            let reader = super::super::get_reader(storage)?;
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
                        Value::string(e.edge_type_name),
                        Value::BigInt(e.edge_type_id as i64),
                        Value::string(e.src_tag_name),
                        Value::string(e.dst_tag_name),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
    };
    result
}
