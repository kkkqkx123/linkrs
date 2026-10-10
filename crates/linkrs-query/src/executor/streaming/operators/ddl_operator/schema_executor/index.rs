use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::IndexManageCommand;
use crate::storage::StorageSchemaOps;
use linkrs_core::error::QueryError;
use linkrs_core::types::index::{Index, IndexConfig, IndexType};
use linkrs_core::Value;

pub(in crate::executor::streaming::operators) fn execute_index_manage(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::IndexManage {
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
        IndexManageCommand::CreateTagIndex { .. } => super::super::exec_ddl(storage, |s| {
            let idx_name = index_name.as_deref().unwrap_or("unnamed");
            let schema = target_name.as_deref().unwrap_or(space_name);
            let fields: Vec<linkrs_core::types::IndexField> = index_properties
                .iter()
                .map(|p| {
                    linkrs_core::types::IndexField::new(
                        p.clone(),
                        linkrs_core::Value::Null(linkrs_core::value::NullType::Null),
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
        IndexManageCommand::CreateEdgeIndex { .. } => super::super::exec_ddl(storage, |s| {
            let idx_name = index_name.as_deref().unwrap_or("unnamed");
            let schema = target_name.as_deref().unwrap_or(space_name);
            let fields: Vec<linkrs_core::types::IndexField> = index_properties
                .iter()
                .map(|p| {
                    linkrs_core::types::IndexField::new(
                        p.clone(),
                        linkrs_core::Value::Null(linkrs_core::value::NullType::Null),
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
        IndexManageCommand::DropTagIndex { .. } => super::super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DropEdgeIndex { .. } => super::super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_edge_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DescTagIndex { .. } | IndexManageCommand::ShowCreateIndex { .. } => {
            let reader = super::super::get_reader(storage)?;
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
                    Ok(Some(super::super::make_single_row(
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
                None => Ok(Some(super::super::make_manage_result(
                    "desc_index",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        IndexManageCommand::ShowIndexes | IndexManageCommand::ShowTagIndexes => {
            let reader = super::super::get_reader(storage)?;
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
        IndexManageCommand::RebuildTagIndex { .. } => super::super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::rebuild_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageCommand::DescEdgeIndex { .. } => {
            let reader = super::super::get_reader(storage)?;
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
                    Ok(Some(super::super::make_single_row(
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
                None => Ok(Some(super::super::make_manage_result(
                    "desc_index",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        IndexManageCommand::ShowEdgeIndexes => {
            let reader = super::super::get_reader(storage)?;
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
        IndexManageCommand::RebuildEdgeIndex { .. } => super::super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::rebuild_edge_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
    };
    result
}

pub(in crate::executor::streaming::operators) fn execute_delete_index(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::DeleteIndex {
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
    super::super::exec_ddl(storage, |schema| {
        // Try tag index first, then edge index
        if StorageSchemaOps::drop_tag_index(schema, space_name, index_name).is_ok() {
            return Ok(());
        }
        StorageSchemaOps::drop_edge_index(schema, space_name, index_name)
            .map(|_| ())
            .map_err(|error| QueryError::execution(error.to_string()))
    })
}
