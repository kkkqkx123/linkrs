use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::TagManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::tag::TagInfo;
use super::common::render_create_tag;
use linkrs_core::Value;

pub(in crate::executor::streaming::operators) fn execute_tag_manage(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::TagManage {
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
    let tag_name = match command {
        TagManageCommand::Create { tag_name, .. }
        | TagManageCommand::Alter { tag_name, .. }
        | TagManageCommand::Rename {
            old_name: tag_name, ..
        }
        | TagManageCommand::Desc { tag_name }
        | TagManageCommand::ShowCreate { tag_name } => Some(tag_name.clone()),
        TagManageCommand::Drop { .. } => None,
        TagManageCommand::Show => None,
    };
    let result = match command {
        TagManageCommand::Create {
            tag_name,
            properties,
            if_not_exists,
            ttl_duration,
            ttl_col,
        } => super::super::exec_ddl(storage, |s| {
            linkrs_core::types::ttl::validate_ttl(properties, *ttl_duration, ttl_col.as_deref())
                .map_err(QueryError::execution)?;
            let mut info = TagInfo::new(tag_name.clone());
            info.properties = properties.clone();
            info.ttl_duration = *ttl_duration;
            info.ttl_col = ttl_col.clone();
            match StorageSchemaOps::create_tag(s, space_name, &info) {
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
        TagManageCommand::Drop {
            tag_names,
            if_exists,
        } => {
            if tag_names.is_empty() {
                return Err(QueryError::execution(
                    "DROP TAG requires at least one tag name".to_string(),
                ));
            }
            let Some(lock) = storage else {
                return Ok(Some(super::super::make_manage_result(
                    "drop_tag",
                    None,
                    "no-storage",
                )));
            };
            let mut writer = lock.write();
            let mut items = Vec::with_capacity(tag_names.len());
            for name in tag_names {
                match StorageSchemaOps::drop_tag(&mut *writer, space_name, name) {
                    Ok(true) => items.push((name.clone(), "dropped")),
                    Ok(false) if *if_exists => items.push((name.clone(), "not_exists")),
                    Ok(false) => {
                        return Err(QueryError::execution(format!(
                            "Tag '{}' does not exist",
                            name
                        )));
                    }
                    Err(e) => return Err(QueryError::execution(e.to_string())),
                }
            }
            Ok(Some(super::super::make_manage_results("drop_tag", items)))
        }
        TagManageCommand::Alter {
            tag_name,
            additions,
            deletions,
            changes,
        } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::alter_tag(
                s,
                space_name,
                tag_name,
                additions.clone(),
                deletions.clone(),
            )
            .map_err(|e| QueryError::execution(e.to_string()))?;
            for change in changes {
                StorageSchemaOps::rename_tag_property(
                    s,
                    space_name,
                    tag_name,
                    &change.old_name,
                    &change.new_name,
                )
                .map_err(|e| QueryError::execution(e.to_string()))?;
            }
            Ok(())
        }),
        TagManageCommand::Rename { old_name, new_name } => super::super::exec_ddl(storage, |s| {
            StorageSchemaOps::rename_tag(s, space_name, old_name, new_name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        TagManageCommand::Desc { .. } => {
            let reader = super::super::get_reader(storage)?;
            let name = tag_name.as_deref().unwrap_or("");
            match reader
                .get_tag(space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(tag) => {
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
                    let rows: Vec<Vec<Value>> = tag
                        .properties
                        .iter()
                        .map(|p| {
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
                        })
                        .collect();
                    Ok(Some(DataChunk::new(rows, schema)))
                }
                None => {
                    let schema = super::super::make_single_col_schema("Field", "string");
                    Ok(Some(DataChunk::new(vec![], schema)))
                }
            }
        }
        TagManageCommand::Show => {
            let reader = super::super::get_reader(storage)?;
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
                        Value::string(t.tag_name),
                        Value::BigInt(t.tag_id as i64),
                        Value::string(props_str),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        TagManageCommand::ShowCreate { .. } => {
            let reader = super::super::get_reader(storage)?;
            let name = tag_name.as_deref().unwrap_or("");
            match reader
                .get_tag(space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?
            {
                Some(tag) => {
                    let ddl = render_create_tag(
                        &tag.tag_name,
                        &tag.properties,
                        tag.ttl_duration,
                        tag.ttl_col.as_deref(),
                    );
                    let schema =
                        super::super::make_single_col_schema("create_statement", "string");
                    Ok(Some(super::super::make_single_row(
                        schema,
                        vec![Value::string(ddl)],
                    )))
                }
                None => Ok(Some(super::super::make_manage_result(
                    "show_create_tag",
                    Some(name),
                    "not-found",
                ))),
            }
        }
    };
    result
}
