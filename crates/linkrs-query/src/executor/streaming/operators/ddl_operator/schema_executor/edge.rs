use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::EdgeManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::edge::EdgeTypeInfo;
use linkrs_core::Value;

use super::common::{endpoint_rows, render_create_edge};

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
    let result = match command {
        EdgeManageCommand::Create {
            edge_name,
            properties,
            src_tag_name,
            dst_tag_name,
            if_not_exists,
            ttl_duration,
            ttl_col,
        } => super::super::exec_ddl(storage, |s| {
            linkrs_core::types::ttl::validate_ttl(properties, *ttl_duration, ttl_col.as_deref())
                .map_err(QueryError::execution)?;
            let mut info = EdgeTypeInfo::new(edge_name.clone());
            info.properties = properties.clone();
            info.ttl_duration = *ttl_duration;
            info.ttl_col = ttl_col.clone();
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
            edge_names,
            if_exists,
        } => {
            if edge_names.is_empty() {
                return Err(QueryError::execution(
                    "DROP EDGE requires at least one edge type name".to_string(),
                ));
            }
            let Some(lock) = storage else {
                return Ok(Some(super::super::make_manage_result(
                    "drop_edge",
                    None,
                    "no-storage",
                )));
            };
            let mut writer = lock.write();
            let mut items = Vec::with_capacity(edge_names.len());
            for name in edge_names {
                match StorageSchemaOps::drop_edge_type(&mut *writer, space_name, name) {
                    Ok(true) => items.push((name.clone(), "dropped")),
                    Ok(false) if *if_exists => items.push((name.clone(), "not_exists")),
                    Ok(false) => {
                        return Err(QueryError::execution(format!(
                            "Edge type '{}' does not exist",
                            name
                        )));
                    }
                    Err(e) => return Err(QueryError::execution(e.to_string())),
                }
            }
            Ok(Some(super::super::make_manage_results("drop_edge", items)))
        }
        EdgeManageCommand::Alter {
            edge_name,
            additions,
            deletions,
            changes,
        } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::alter_edge_type(
                s,
                space_name,
                edge_name,
                additions.clone(),
                deletions.clone(),
            )
            .map_err(|e| QueryError::execution(e.to_string()))?;
            for change in changes {
                if !StorageSchemaOps::rename_edge_property(
                    s,
                    space_name,
                    edge_name,
                    &change.old_name,
                    &change.new_name,
                )
                .map_err(|e| QueryError::execution(e.to_string()))?
                {
                    return Err(QueryError::execution(format!(
                        "Edge property '{}' does not exist on edge type '{}'",
                        change.old_name, edge_name
                    )));
                }
            }
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
        EdgeManageCommand::Desc { edge_name } => {
            let reader = super::super::get_reader(storage)?;
            let name = edge_name.as_str();
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
        EdgeManageCommand::ShowCreate { edge_name } => {
            let reader = super::super::get_reader(storage)?;
            let name = edge_name.as_str();
            match reader
                .get_edge_type(space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(et) => {
                    let ddl = render_create_edge(
                        &et.edge_type_name,
                        &et.properties,
                        &et.src_tag_name,
                        &et.dst_tag_name,
                        et.ttl_duration,
                        et.ttl_col.as_deref(),
                    );
                    let schema =
                        super::super::make_single_col_schema("create_statement", "string");
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![Value::string(ddl)],
                    )))
                }
                None => Ok(Some(super::super::make_manage_result(
                    "show_create_edge",
                    Some(name),
                    "not-found",
                ))),
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
