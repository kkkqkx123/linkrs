use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::edge::EdgeTypeInfo;
use crate::core::types::index::{Index, IndexConfig, IndexType};
use crate::core::types::space::SpaceInfo;
use crate::core::types::tag::TagInfo;
use crate::core::types::PropertyDef;
use crate::core::{NullType, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
};
use crate::storage::{QueryStorage, StorageSchemaOps};

pub(super) fn execute_space_manage(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    command: &SpaceManageNode,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let space_name = extract_space_manage_name(command);
    let result = match command {
        SpaceManageNode::Create(_) => super::exec_ddl(storage, |s| {
            let mut info = SpaceInfo::new(space_name.clone().unwrap_or_default());
            StorageSchemaOps::create_space(s, &mut info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageNode::Drop(_) => super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageNode::Alter(_) => {
            let comment = space_name.as_deref().unwrap_or("");
            super::exec_ddl(storage, |s| {
                StorageSchemaOps::alter_space_comment(s, 0, comment.to_string())
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                Ok(())
            })
        }
        SpaceManageNode::Clear(_) => super::exec_ddl(storage, |s| {
            let name = space_name.as_deref().unwrap_or("");
            StorageSchemaOps::clear_space(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        SpaceManageNode::Desc(_) | SpaceManageNode::ShowCreate(_) => {
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
                None => Ok(Some(super::make_manage_result(
                    "desc_space",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        SpaceManageNode::Switch(_) => {
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
                            Value::String(info.space_name),
                            Value::BigInt(info.space_id as i64),
                            Value::String(format!("{:?}", info.vid_type)),
                        ],
                    )))
                }
                None => Err(QueryError::execution(format!("Space not found: {}", name))),
            }
        }
        SpaceManageNode::Show(_) => {
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
                        Value::String(info.space_name),
                        Value::BigInt(info.space_id as i64),
                        Value::String(format!("{:?}", info.vid_type)),
                        Value::Int(info.partition_num),
                        Value::Int(info.replica_factor),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
    };
    base.lifecycle.mark_closed();
    result
}

pub(super) fn execute_tag_manage(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    command: &TagManageNode,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let tag_name = extract_tag_manage_name(command);
    let properties = extract_tag_manage_properties(command);
    let result = match command {
        TagManageNode::Create(_) => super::exec_ddl(storage, |s| {
            let name = tag_name.as_deref().unwrap_or("unnamed");
            let mut info = TagInfo::new(name.to_string());
            info.properties = properties.clone();
            StorageSchemaOps::create_tag(s, space_name, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        TagManageNode::Drop(_) => super::exec_ddl(storage, |s| {
            let name = tag_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_tag(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        TagManageNode::Alter(_) => super::exec_ddl(storage, |s| {
            let name = tag_name.as_deref().unwrap_or("");
            StorageSchemaOps::alter_tag(s, space_name, name, vec![], vec![])
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        TagManageNode::Desc(_) => {
            let reader = super::get_reader(storage)?;
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
                    Ok(Some(super::make_single_row(
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
                None => Ok(Some(super::make_manage_result(
                    "desc_tag",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        TagManageNode::Show(_) => {
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
                        Value::String(t.tag_name),
                        Value::BigInt(t.tag_id as i64),
                        Value::String(props_str),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        TagManageNode::ShowCreate(_) => {
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
                        vec![Value::String(ddl)],
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
    base.lifecycle.mark_closed();
    result
}

pub(super) fn execute_edge_manage(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    command: &EdgeManageNode,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let edge_type = extract_edge_manage_name(command);
    let properties = extract_edge_manage_properties(command);
    let result = match command {
        EdgeManageNode::Create(_) => super::exec_ddl(storage, |s| {
            let et = edge_type.as_deref().unwrap_or("unnamed");
            let mut info = EdgeTypeInfo::new(et.to_string());
            info.properties = properties.clone();
            StorageSchemaOps::create_edge_type(s, space_name, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        EdgeManageNode::Drop(_) => super::exec_ddl(storage, |s| {
            let name = edge_type.as_deref().unwrap_or("");
            StorageSchemaOps::drop_edge_type(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        EdgeManageNode::Alter(_) => super::exec_ddl(storage, |s| {
            let name = edge_type.as_deref().unwrap_or("");
            StorageSchemaOps::alter_edge_type(s, space_name, name, vec![], vec![])
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        EdgeManageNode::Desc(_) | EdgeManageNode::ShowCreate(_) => {
            let reader = super::get_reader(storage)?;
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
                    Ok(Some(super::make_single_row(
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
                None => Ok(Some(super::make_manage_result(
                    "desc_edge",
                    Some(name),
                    "not-found",
                ))),
            }
        }
        EdgeManageNode::Show(_) => {
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
                        Value::String(e.edge_type_name),
                        Value::BigInt(e.edge_type_id as i64),
                        Value::String(e.src_tag_name),
                        Value::String(e.dst_tag_name),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
    };
    base.lifecycle.mark_closed();
    result
}

pub(super) fn execute_index_manage(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    command: &IndexManageNode,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let index_name = extract_index_manage_name(command);
    let result = match command {
        IndexManageNode::CreateTagIndex(_) => super::exec_ddl(storage, |s| {
            let idx_name = index_name.as_deref().unwrap_or("unnamed");
            let info = Index::new(IndexConfig {
                id: 0,
                name: idx_name.to_string(),
                space_id: 0,
                schema_name: space_name.to_string(),
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
        IndexManageNode::CreateEdgeIndex(_) => Err(QueryError::execution(
            "Edge index creation is not exposed by StorageSchemaOps".to_string(),
        )),
        IndexManageNode::DropTagIndex(_) => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::drop_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageNode::DropEdgeIndex(_) => Err(QueryError::execution(
            "Edge index deletion is not exposed by StorageSchemaOps".to_string(),
        )),
        IndexManageNode::DescTagIndex(_) | IndexManageNode::ShowCreateIndex(_) => {
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
                            Value::String(idx.name),
                            Value::String(format!("{:?}", idx.index_type)),
                            Value::String(fields_str),
                            Value::String(format!("{:?}", idx.status)),
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
        IndexManageNode::ShowIndexes(_) | IndexManageNode::ShowTagIndexes(_) => {
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
                        Value::String(idx.name),
                        Value::String(format!("{:?}", idx.index_type)),
                        Value::String(fields_str),
                        Value::String(format!("{:?}", idx.status)),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
        IndexManageNode::RebuildTagIndex(_) => super::exec_ddl(storage, |s| {
            let name = index_name.as_deref().unwrap_or("");
            StorageSchemaOps::rebuild_tag_index(s, space_name, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        IndexManageNode::DescEdgeIndex(_)
        | IndexManageNode::ShowEdgeIndexes(_)
        | IndexManageNode::RebuildEdgeIndex(_) => Err(QueryError::execution(
            "Edge index management is not exposed by StorageSchemaOps".to_string(),
        )),
    };
    base.lifecycle.mark_closed();
    result
}

pub(super) fn execute_delete_index(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    index_name: &str,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = super::exec_ddl(storage, |schema| {
        StorageSchemaOps::drop_tag_index(schema, space_name, index_name)
            .map(|_| ())
            .map_err(|error| QueryError::execution(error.to_string()))
    });
    base.lifecycle.mark_closed();
    result
}

fn extract_space_manage_name(node: &SpaceManageNode) -> Option<String> {
    use SpaceManageNode::*;
    match node {
        Create(node) => Some(node.info().space_name.clone()),
        Drop(node) => Some(node.space_name().to_string()),
        Desc(node) => Some(node.space_name().to_string()),
        ShowCreate(node) => Some(node.space_name().to_string()),
        Switch(node) => Some(node.space_name().to_string()),
        Clear(node) => Some(node.space_name().to_string()),
        Alter(node) => Some(node.space_name().to_string()),
        Show(_) => None,
    }
}

fn extract_tag_manage_name(node: &TagManageNode) -> Option<String> {
    use TagManageNode::*;
    match node {
        Create(node) => Some(node.info().tag_name.clone()),
        Alter(node) => Some(node.info().tag_name.clone()),
        Desc(node) => Some(node.tag_name().to_string()),
        Drop(node) => Some(node.tag_name().to_string()),
        ShowCreate(node) => Some(node.tag_name().to_string()),
        Show(_) => None,
    }
}

fn extract_tag_manage_properties(node: &TagManageNode) -> Vec<PropertyDef> {
    match node {
        TagManageNode::Create(node) => node.info().properties.clone(),
        _ => Vec::new(),
    }
}

fn extract_edge_manage_name(node: &EdgeManageNode) -> Option<String> {
    use EdgeManageNode::*;
    match node {
        Create(node) => Some(node.info().edge_name.clone()),
        Alter(node) => Some(node.info().edge_name.clone()),
        Desc(node) => Some(node.edge_name().to_string()),
        Drop(node) => Some(node.edge_name().to_string()),
        ShowCreate(node) => Some(node.edge_name().to_string()),
        Show(_) => None,
    }
}

fn extract_edge_manage_properties(node: &EdgeManageNode) -> Vec<PropertyDef> {
    match node {
        EdgeManageNode::Create(node) => node.info().properties.clone(),
        _ => Vec::new(),
    }
}

fn extract_index_manage_name(node: &IndexManageNode) -> Option<String> {
    use IndexManageNode::*;
    match node {
        CreateTagIndex(node) => Some(node.info().index_name.clone()),
        DropTagIndex(node) => Some(node.index_name().to_string()),
        DescTagIndex(node) => Some(node.index_name().to_string()),
        RebuildTagIndex(node) => Some(node.index_name().to_string()),
        CreateEdgeIndex(node) => Some(node.info().index_name.clone()),
        DropEdgeIndex(node) => Some(node.index_name().to_string()),
        DescEdgeIndex(node) => Some(node.index_name().to_string()),
        RebuildEdgeIndex(node) => Some(node.index_name().to_string()),
        ShowCreateIndex(node) => Some(node.index_name().to_string()),
        ShowTagIndexes(_) | ShowEdgeIndexes(_) | ShowIndexes(_) => None,
    }
}
