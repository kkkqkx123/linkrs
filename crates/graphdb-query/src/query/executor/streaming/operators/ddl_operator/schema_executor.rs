use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::edge::EdgeTypeInfo;
use crate::core::types::index::{Index, IndexConfig, IndexType};
use crate::core::types::space::SpaceInfo;
use crate::core::types::tag::TagInfo;
use crate::core::{NullType, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::operators::spec::{
    EdgeManageCommand, IndexManageCommand, SpaceManageCommand, TagManageCommand,
};
use crate::storage::StorageSchemaOps;

pub(super) fn execute_space_manage(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::SpaceManage {
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
        | SpaceManageCommand::Clear { space_name } => Some(space_name.clone()),
        SpaceManageCommand::Show => None,
    };
    let result = match command {
        SpaceManageCommand::Create {
            space_name,
            vid_type,
        } => super::exec_ddl(storage, |s| {
            let vid_type = parse_vid_type_str(vid_type);
            let mut space_info = SpaceInfo::new(space_name.clone()).with_vid_type(vid_type);
            StorageSchemaOps::create_space(s, &mut space_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Drop { .. } => super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Alter { .. } => {
            let comment = space_name.as_deref().unwrap_or("");
            super::exec_ddl(storage, |s| {
                StorageSchemaOps::alter_space_comment(s, 0, comment.to_string())
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                Ok(())
            })
        }
        SpaceManageCommand::Clear { .. } => super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::clear_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageCommand::Desc { .. } | SpaceManageCommand::ShowCreate { .. } => {
            let reader = super::get_reader(storage)?;
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
                    Ok(Some(super::make_single_row(
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
                None => Ok(Some(super::make_manage_result(
                    "desc_space",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        SpaceManageCommand::Switch { .. } => {
            let reader = super::get_reader(storage)?;
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
                    Ok(Some(super::make_single_row(
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
            let reader = super::get_reader(storage)?;
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
    };
    result
}

pub(super) fn execute_tag_manage(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::TagManage {
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
        | TagManageCommand::Desc { tag_name }
        | TagManageCommand::Drop { tag_name, .. }
        | TagManageCommand::ShowCreate { tag_name } => Some(tag_name.clone()),
        TagManageCommand::Show => None,
    };
    let result = match command {
        TagManageCommand::Create {
            tag_name,
            properties,
            if_not_exists,
        } => super::exec_ddl(storage, |s| {
            let mut info = TagInfo::new(tag_name.clone());
            info.properties = properties.clone();
            match StorageSchemaOps::create_tag(s, space_name, &info) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_not_exists
                        && e.kind()
                            == crate::core::error::storage::StorageErrorKind::LabelAlreadyExists =>
                {
                    Ok(())
                }
                Err(e) => Err(QueryError::execution(e.to_string())),
            }
        }),
        TagManageCommand::Drop {
            tag_name,
            if_exists,
        } => super::exec_ddl(storage, |s| {
            match StorageSchemaOps::drop_tag(s, space_name, tag_name) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_exists
                        && (e.kind()
                            == crate::core::error::storage::StorageErrorKind::LabelNotFound
                            || e.kind()
                                == crate::core::error::storage::StorageErrorKind::NotFound) =>
                {
                    Ok(())
                }
                Err(e) => Err(QueryError::execution(e.to_string())),
            }
        }),
        TagManageCommand::Alter {
            tag_name,
            additions,
            deletions,
            changes,
        } => super::exec_ddl(storage, |s| {
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
        TagManageCommand::Desc { .. } => {
            let reader = super::get_reader(storage)?;
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
                    let schema = super::make_single_col_schema("Field", "string");
                    Ok(Some(DataChunk::new(vec![], schema)))
                }
            }
        }
        TagManageCommand::Show => {
            let reader = super::get_reader(storage)?;
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
            let reader = super::get_reader(storage)?;
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
                    let schema = super::make_single_col_schema("create_tag", "string");
                    Ok(Some(super::make_single_row(
                        schema,
                        vec![Value::string(ddl)],
                    )))
                }
                None => Ok(Some(super::make_manage_result(
                    "show_create_tag",
                    Some(name),
                    "not-found",
                ))),
            }
        }
    };
    result
}

pub(super) fn execute_edge_manage(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::EdgeManage {
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
        } => super::exec_ddl(storage, |s| {
            let mut info = EdgeTypeInfo::new(edge_name.clone());
            info.properties = properties.clone();
            info.src_tag_name = src_tag_name.clone().unwrap_or_default();
            info.dst_tag_name = dst_tag_name.clone().unwrap_or_default();
            match StorageSchemaOps::create_edge_type(s, space_name, &info) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_not_exists
                        && e.kind()
                            == crate::core::error::storage::StorageErrorKind::LabelAlreadyExists =>
                {
                    Ok(())
                }
                Err(e) => Err(QueryError::execution(e.to_string())),
            }
        }),
        EdgeManageCommand::Drop {
            edge_name,
            if_exists,
        } => super::exec_ddl(storage, |s| {
            match StorageSchemaOps::drop_edge_type(s, space_name, edge_name) {
                Ok(_) => Ok(()),
                Err(e)
                    if *if_exists
                        && (e.kind()
                            == crate::core::error::storage::StorageErrorKind::LabelNotFound
                            || e.kind()
                                == crate::core::error::storage::StorageErrorKind::NotFound) =>
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
        } => super::exec_ddl(storage, |s| {
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
        EdgeManageCommand::Desc { .. } | EdgeManageCommand::ShowCreate { .. } => {
            let reader = super::get_reader(storage)?;
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
                    let rows: Vec<Vec<Value>> = et
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
                    let schema = super::make_single_col_schema("Field", "string");
                    Ok(Some(DataChunk::new(vec![], schema)))
                }
            }
        }
        EdgeManageCommand::Show => {
            let reader = super::get_reader(storage)?;
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

pub(super) fn execute_index_manage(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::IndexManage {
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
    let index_name = match command {
        IndexManageCommand::CreateTagIndex { index_name, .. }
        | IndexManageCommand::DropTagIndex { index_name }
        | IndexManageCommand::DescTagIndex { index_name }
        | IndexManageCommand::RebuildTagIndex { index_name }
        | IndexManageCommand::CreateEdgeIndex { index_name, .. }
        | IndexManageCommand::DropEdgeIndex { index_name }
        | IndexManageCommand::DescEdgeIndex { index_name }
        | IndexManageCommand::RebuildEdgeIndex { index_name }
        | IndexManageCommand::ShowCreateIndex { index_name } => Some(index_name.clone()),
        IndexManageCommand::ShowTagIndexes
        | IndexManageCommand::ShowEdgeIndexes
        | IndexManageCommand::ShowIndexes => None,
    };
    let target_name = match command {
        IndexManageCommand::CreateTagIndex { target_name, .. }
        | IndexManageCommand::CreateEdgeIndex { target_name, .. } => {
            Some(target_name.clone()).filter(|s| !s.is_empty())
        }
        _ => None,
    };
    let index_properties = match command {
        IndexManageCommand::CreateTagIndex { properties, .. }
        | IndexManageCommand::CreateEdgeIndex { properties, .. } => properties.clone(),
        _ => Vec::new(),
    };

    // Resolve space_id from space_name to avoid space ID mismatch
    let resolved_space_id = storage
        .as_ref()
        .and_then(|lock| lock.read().get_space_id(space_name).ok())
        .unwrap_or(0);

    let result = match command {
        IndexManageCommand::CreateTagIndex { .. } => super::exec_ddl(storage, |s| {
            let idx_name = index_name.as_deref().unwrap_or("unnamed");
            let schema = target_name.as_deref().unwrap_or(space_name);
            let fields: Vec<crate::core::types::IndexField> = index_properties
                .iter()
                .map(|p| {
                    crate::core::types::IndexField::new(
                        p.clone(),
                        crate::core::Value::Null(crate::core::value::NullType::Null),
                        true,
                    )
                })
                .collect();
            let info = Index::new(IndexConfig {
                id: 0,
                name: idx_name.to_string(),
                space_id: resolved_space_id,
                schema_name: schema.to_string(),
                fields,
                properties: index_properties.clone(),
                index_type: IndexType::TagIndex,
                is_unique: false,
                covering: false,
                partial_condition: None,
            });
            StorageSchemaOps::create_tag_index(s, space_name, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::CreateEdgeIndex { .. } => super::exec_ddl(storage, |s| {
            let idx_name = index_name.as_deref().unwrap_or("unnamed");
            let schema = target_name.as_deref().unwrap_or(space_name);
            let fields: Vec<crate::core::types::IndexField> = index_properties
                .iter()
                .map(|p| {
                    crate::core::types::IndexField::new(
                        p.clone(),
                        crate::core::Value::Null(crate::core::value::NullType::Null),
                        true,
                    )
                })
                .collect();
            let info = Index::new(IndexConfig {
                id: 0,
                name: idx_name.to_string(),
                space_id: resolved_space_id,
                schema_name: schema.to_string(),
                fields,
                properties: index_properties,
                index_type: IndexType::EdgeIndex,
                is_unique: false,
                covering: false,
                partial_condition: None,
            });
            StorageSchemaOps::create_edge_index(s, space_name, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DropTagIndex { .. } => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DropEdgeIndex { .. } => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_edge_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DescTagIndex { .. } | IndexManageCommand::ShowCreateIndex { .. } => {
            let reader = super::get_reader(storage)?;
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
                    Ok(Some(super::make_single_row(
                        schema,
                        vec![
                            Value::string(idx.name),
                            Value::string(format!("{:?}", idx.index_type)),
                            Value::string(fields_str),
                            Value::string(format!("{:?}", idx.status)),
                            Value::Bool(idx.is_unique),
                        ],
                    )))
                }
                None => Ok(Some(super::make_manage_result(
                    "desc_index",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        IndexManageCommand::ShowIndexes | IndexManageCommand::ShowTagIndexes => {
            let reader = super::get_reader(storage)?;
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
                        Value::string(idx.name),
                        Value::string(format!("{:?}", idx.index_type)),
                        Value::string(fields_str),
                        Value::string(format!("{:?}", idx.status)),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        IndexManageCommand::RebuildTagIndex { .. } => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::rebuild_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DescEdgeIndex { .. } => {
            let reader = super::get_reader(storage)?;
            let name = index_name.as_deref().unwrap_or("");
            match reader
                .get_edge_index(space_name, name)
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
                    Ok(Some(super::make_single_row(
                        schema,
                        vec![
                            Value::string(idx.name),
                            Value::string(format!("{:?}", idx.index_type)),
                            Value::string(fields_str),
                            Value::string(format!("{:?}", idx.status)),
                            Value::Bool(idx.is_unique),
                        ],
                    )))
                }
                None => Ok(Some(super::make_manage_result(
                    "desc_index",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        IndexManageCommand::ShowEdgeIndexes => {
            let reader = super::get_reader(storage)?;
            let indexes = reader
                .list_edge_indexes(space_name)
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
                        Value::string(idx.name),
                        Value::string(format!("{:?}", idx.index_type)),
                        Value::string(fields_str),
                        Value::string(format!("{:?}", idx.status)),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        IndexManageCommand::RebuildEdgeIndex { .. } => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::rebuild_edge_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
    };
    result
}

pub(super) fn execute_delete_index(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::DeleteIndex {
        storage,
        space_name,
        index_name,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = super::exec_ddl(storage, |schema| {
        // Try tag index first, then edge index
        if StorageSchemaOps::drop_tag_index(schema, space_name, index_name).is_ok() {
            return Ok(());
        }
        StorageSchemaOps::drop_edge_index(schema, space_name, index_name)
            .map(|_| ())
            .map_err(|error| QueryError::execution(error.to_string()))
    });
    result
}

fn parse_vid_type_str(s: &str) -> crate::core::types::DataType {
    let upper = s.trim().to_uppercase();
    if upper == "INT64" {
        crate::core::types::DataType::BigInt
    } else if upper == "INT32" {
        crate::core::types::DataType::Int
    } else if upper == "INT16" || upper == "INT8" {
        crate::core::types::DataType::SmallInt
    } else if upper == "STRING" {
        crate::core::types::DataType::String
    } else if upper == "VID" {
        crate::core::types::DataType::VID
    } else if upper.starts_with("FIXED_STRING(") || upper.starts_with("FIXEDSTRING(") {
        let inner = upper
            .trim_start_matches("FIXED_STRING(")
            .trim_start_matches("FIXEDSTRING(")
            .trim_end_matches(')');
        if let Ok(n) = inner.parse::<usize>() {
            crate::core::types::DataType::FixedString(n)
        } else {
            crate::core::types::DataType::FixedString(32)
        }
    } else {
        crate::core::types::DataType::String
    }
}
