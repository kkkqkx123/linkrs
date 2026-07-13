use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::permission::RoleType;
use crate::core::types::edge::EdgeTypeInfo;

use crate::core::types::index::{Index, IndexConfig, IndexType};
use crate::core::types::space::SpaceInfo;
use crate::core::types::tag::TagInfo;
use crate::core::types::user::{PasswordInfo, UserAlterInfo, UserInfo};
use crate::core::types::PropertyDef;
use crate::core::{NullType, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode, UserManageNode,
};
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
) -> Result<parking_lot::RwLockReadGuard<'_, dyn StorageClient>, QueryError> {
    storage
        .as_ref()
        .map(|s| s.read())
        .ok_or_else(|| QueryError::execution("No storage available".to_string()))
}

pub(super) fn make_single_row(schema: Arc<Schema>, cols: Vec<Value>) -> DataChunk {
    DataChunk::new(vec![cols], schema)
}

fn make_single_col_schema(col_name: &str, col_type: &str) -> Arc<Schema> {
    Arc::new(Schema::new(vec![ColumnInfo {
        name: col_name.to_string(),
        data_type: col_type.to_string(),
    }]))
}

#[derive(Debug)]
pub enum DdlOperator {
    SpaceManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        command: SpaceManageNode,
        emitted: bool,
    },
    TagManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        command: TagManageNode,
        emitted: bool,
    },
    EdgeManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        command: EdgeManageNode,
        emitted: bool,
    },
    IndexManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        command: IndexManageNode,
        emitted: bool,
    },
    DeleteIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        emitted: bool,
    },
    UserManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        command: UserManageNode,
        emitted: bool,
    },
    ShowStats {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        emitted: bool,
    },
    Analyze {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        analyze_target: String,
        target_name: Option<String>,
        emitted: bool,
    },
    Migrate {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        migration_data: Option<String>,
        emitted: bool,
    },
}

impl DdlOperator {
    /// Create a DdlOperator from an immutable spec.
    pub fn from_spec(
        spec: &super::super::operator_spec::DdlSpec,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
    ) -> Self {
        match spec {
            super::super::operator_spec::DdlSpec::SpaceManage { command } => {
                DdlOperator::SpaceManage {
                    storage: storage.clone(),
                    command: command.clone(),
                    emitted: false,
                }
            }
            super::super::operator_spec::DdlSpec::TagManage {
                space_name,
                command,
            } => DdlOperator::TagManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::super::operator_spec::DdlSpec::EdgeManage {
                space_name,
                command,
            } => DdlOperator::EdgeManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::super::operator_spec::DdlSpec::IndexManage {
                space_name,
                command,
            } => DdlOperator::IndexManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::super::operator_spec::DdlSpec::DeleteIndex {
                space_name,
                index_name,
            } => DdlOperator::DeleteIndex {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                emitted: false,
            },
            super::super::operator_spec::DdlSpec::UserManage { command } => {
                DdlOperator::UserManage {
                    storage: storage.clone(),
                    command: command.clone(),
                    emitted: false,
                }
            }
            super::super::operator_spec::DdlSpec::ShowStats { space_name } => {
                DdlOperator::ShowStats {
                    storage: storage.clone(),
                    space_name: space_name.clone(),
                    emitted: false,
                }
            }
            super::super::operator_spec::DdlSpec::Analyze { space_name } => DdlOperator::Analyze {
                storage: storage.clone(),
                space_name: space_name.clone(),
                analyze_target: String::new(),
                target_name: None,
                emitted: false,
            },
            super::super::operator_spec::DdlSpec::Migrate {
                space_name,
                action,
                migration_data,
            } => DdlOperator::Migrate {
                storage,
                space_name: space_name.clone(),
                action: action.clone(),
                migration_data: migration_data.clone(),
                emitted: false,
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            DdlOperator::SpaceManage { .. }
            | DdlOperator::TagManage { .. }
            | DdlOperator::EdgeManage { .. }
            | DdlOperator::IndexManage { .. }
            | DdlOperator::DeleteIndex { .. }
            | DdlOperator::UserManage { .. }
            | DdlOperator::ShowStats { .. }
            | DdlOperator::Analyze { .. }
            | DdlOperator::Migrate { .. } => {
                input.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            DdlOperator::SpaceManage {
                storage,
                command,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                let space_name = extract_space_manage_name(command);
                let result = match command {
                    SpaceManageNode::Create(_) => exec_ddl(storage, |s| {
                        let mut info = SpaceInfo::new(space_name.clone().unwrap_or_default());
                        StorageSchemaOps::create_space(s, &mut info)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    SpaceManageNode::Drop(_) => exec_ddl(storage, |s| {
                        let name = space_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_space(s, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    SpaceManageNode::Alter(_) => {
                        let comment = space_name.as_deref().unwrap_or("");
                        exec_ddl(storage, |s| {
                            StorageSchemaOps::alter_space_comment(s, 0, comment.to_string())
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                            Ok(())
                        })
                    }
                    SpaceManageNode::Clear(_) => exec_ddl(storage, |s| {
                        let name = space_name.as_deref().unwrap_or("");
                        StorageSchemaOps::clear_space(s, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    SpaceManageNode::Desc(_) | SpaceManageNode::ShowCreate(_) => {
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
                    SpaceManageNode::Switch(_) => {
                        let reader = get_reader(storage)?;
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
                                Ok(Some(make_single_row(
                                    schema,
                                    vec![
                                        Value::String(info.space_name),
                                        Value::BigInt(info.space_id as i64),
                                        Value::String(format!("{:?}", info.vid_type)),
                                    ],
                                )))
                            }
                            None => {
                                Err(QueryError::execution(format!("Space not found: {}", name)))
                            }
                        }
                    }
                    SpaceManageNode::Show(_) => {
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

            DdlOperator::TagManage {
                storage,
                space_name,
                command,
                emitted,
            } => {
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
                    TagManageNode::Create(_) => exec_ddl(storage, |s| {
                        let name = tag_name.as_deref().unwrap_or("unnamed");
                        let mut info = TagInfo::new(name.to_string());
                        info.properties = properties.clone();
                        StorageSchemaOps::create_tag(s, space_name, &info)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    TagManageNode::Drop(_) => exec_ddl(storage, |s| {
                        let name = tag_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_tag(s, space_name, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    TagManageNode::Alter(_) => exec_ddl(storage, |s| {
                        let name = tag_name.as_deref().unwrap_or("");
                        StorageSchemaOps::alter_tag(s, space_name, name, vec![], vec![])
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    TagManageNode::Desc(_) => {
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
                    TagManageNode::Show(_) => {
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
                    TagManageNode::ShowCreate(_) => {
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
                };
                base.lifecycle.mark_closed();
                result
            }

            DdlOperator::EdgeManage {
                storage,
                space_name,
                command,
                emitted,
            } => {
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
                    EdgeManageNode::Create(_) => exec_ddl(storage, |s| {
                        let et = edge_type.as_deref().unwrap_or("unnamed");
                        let mut info = EdgeTypeInfo::new(et.to_string());
                        info.properties = properties.clone();
                        StorageSchemaOps::create_edge_type(s, space_name, &info)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    EdgeManageNode::Drop(_) => exec_ddl(storage, |s| {
                        let name = edge_type.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_edge_type(s, space_name, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    EdgeManageNode::Alter(_) => exec_ddl(storage, |s| {
                        let name = edge_type.as_deref().unwrap_or("");
                        StorageSchemaOps::alter_edge_type(s, space_name, name, vec![], vec![])
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    EdgeManageNode::Desc(_) | EdgeManageNode::ShowCreate(_) => {
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
                    EdgeManageNode::Show(_) => {
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
                };
                base.lifecycle.mark_closed();
                result
            }

            DdlOperator::IndexManage {
                storage,
                space_name,
                command,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                let index_name = extract_index_manage_name(command);
                let result = match command {
                    IndexManageNode::CreateTagIndex(_) => exec_ddl(storage, |s| {
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
                    IndexManageNode::CreateEdgeIndex(_) => Err(QueryError::execution(
                        "Edge index creation is not exposed by StorageSchemaOps".to_string(),
                    )),
                    IndexManageNode::DropTagIndex(_) => exec_ddl(storage, |s| {
                        let name = index_name.as_deref().unwrap_or("");
                        StorageSchemaOps::drop_tag_index(s, space_name, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    IndexManageNode::DropEdgeIndex(_) => Err(QueryError::execution(
                        "Edge index deletion is not exposed by StorageSchemaOps".to_string(),
                    )),
                    IndexManageNode::DescTagIndex(_) | IndexManageNode::ShowCreateIndex(_) => {
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
                    IndexManageNode::ShowIndexes(_) | IndexManageNode::ShowTagIndexes(_) => {
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
                    IndexManageNode::RebuildTagIndex(_) => exec_ddl(storage, |s| {
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

            DdlOperator::DeleteIndex {
                storage,
                space_name,
                index_name,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let result = exec_ddl(storage, |schema| {
                    StorageSchemaOps::drop_tag_index(schema, space_name, index_name)
                        .map(|_| ())
                        .map_err(|error| QueryError::execution(error.to_string()))
                });
                base.lifecycle.mark_closed();
                result
            }

            DdlOperator::UserManage {
                storage,
                command,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                let username = extract_user_manage_name(command);
                let result = match command {
                    UserManageNode::Create(_) => exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("unknown");
                        let info = UserInfo::new(name.to_string(), "".to_string())
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        StorageAuthOps::create_user(s, &info)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    UserManageNode::Drop(_) => exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("");
                        StorageAuthOps::drop_user(s, name)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    UserManageNode::Alter(_) => exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("");
                        let alter_info = UserAlterInfo::new(name.to_string());
                        StorageAuthOps::alter_user(s, &alter_info)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    UserManageNode::DescribeUser(_) => {
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
                    UserManageNode::ChangePassword(_) => exec_auth(storage, |s| {
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
                    UserManageNode::GrantRole(_) => exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("");
                        StorageAuthOps::grant_role(s, name, 0, RoleType::User)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    UserManageNode::RevokeRole(_) => exec_auth(storage, |s| {
                        let name = username.as_deref().unwrap_or("");
                        StorageAuthOps::revoke_role(s, name, 0)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        Ok(())
                    }),
                    UserManageNode::ShowRoles(_) | UserManageNode::ShowUsers(_) => {
                        Err(QueryError::execution(
                            "User listing is not exposed by StorageAuthOps".to_string(),
                        ))
                    }
                };
                base.lifecycle.mark_closed();
                result
            }

            DdlOperator::ShowStats {
                storage, emitted, ..
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                base.lifecycle.mark_closed();

                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let stats = reader.get_storage_stats();

                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo {
                            name: "metric".to_string(),
                            data_type: "string".to_string(),
                        },
                        ColumnInfo {
                            name: "value".to_string(),
                            data_type: "string".to_string(),
                        },
                    ]));
                    let rows = vec![
                        vec![
                            Value::String("total_vertices".to_string()),
                            Value::BigInt(stats.total_vertices as i64),
                        ],
                        vec![
                            Value::String("total_edges".to_string()),
                            Value::BigInt(stats.total_edges as i64),
                        ],
                        vec![
                            Value::String("total_spaces".to_string()),
                            Value::BigInt(stats.total_spaces as i64),
                        ],
                        vec![
                            Value::String("total_tags".to_string()),
                            Value::BigInt(stats.total_tags as i64),
                        ],
                        vec![
                            Value::String("total_edge_types".to_string()),
                            Value::BigInt(stats.total_edge_types as i64),
                        ],
                        vec![
                            Value::String("total_size_bytes".to_string()),
                            Value::BigInt(stats.total_size_bytes as i64),
                        ],
                        vec![
                            Value::String("data_size_bytes".to_string()),
                            Value::BigInt(stats.data_size_bytes as i64),
                        ],
                        vec![
                            Value::String("index_size_bytes".to_string()),
                            Value::BigInt(stats.index_size_bytes as i64),
                        ],
                    ];
                    Ok(Some(DataChunk::new(rows, schema)))
                } else {
                    let schema = make_single_col_schema("message", "string");
                    Ok(Some(DataChunk::new(
                        vec![vec![Value::String("no storage available".to_string())]],
                        schema,
                    )))
                }
            }

            DdlOperator::Analyze {
                storage,
                space_name,
                analyze_target,
                target_name,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
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
                base.lifecycle.mark_closed();
                result
            }

            DdlOperator::Migrate {
                storage,
                space_name,
                action,
                migration_data: _migration_data,
                emitted,
            } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                let result = match action.as_str() {
                    "migrate_space" | "migrate" | "migrate_vertex" | "migrate_edge" => {
                        if let Some(lock) = storage {
                            let writer = lock.write();
                            let res = writer.save_to_disk().map_err(|e| {
                                QueryError::execution(format!("Migrate failed: {}", e))
                            });
                            match res {
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
                base.lifecycle.mark_closed();
                result
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            DdlOperator::SpaceManage { .. }
            | DdlOperator::TagManage { .. }
            | DdlOperator::EdgeManage { .. }
            | DdlOperator::IndexManage { .. }
            | DdlOperator::DeleteIndex { .. }
            | DdlOperator::UserManage { .. }
            | DdlOperator::ShowStats { .. }
            | DdlOperator::Analyze { .. }
            | DdlOperator::Migrate { .. } => {
                if base.lifecycle.can_close() {
                    input.stop()?;
                    base.lifecycle.mark_stopped();
                }
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            DdlOperator::SpaceManage { .. }
            | DdlOperator::TagManage { .. }
            | DdlOperator::EdgeManage { .. }
            | DdlOperator::IndexManage { .. }
            | DdlOperator::DeleteIndex { .. }
            | DdlOperator::UserManage { .. }
            | DdlOperator::ShowStats { .. }
            | DdlOperator::Analyze { .. }
            | DdlOperator::Migrate { .. } => {
                if base.lifecycle.can_close() {
                    input.close()?;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
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

fn extract_user_manage_name(node: &UserManageNode) -> Option<String> {
    use UserManageNode::*;
    match node {
        Create(node) => Some(node.username().to_string()),
        Alter(node) => Some(node.username().to_string()),
        Drop(node) => Some(node.username().to_string()),
        DescribeUser(node) => Some(node.username().to_string()),
        GrantRole(node) => Some(node.username().to_string()),
        RevokeRole(node) => Some(node.username().to_string()),
        ChangePassword(_) | ShowRoles(_) | ShowUsers(_) => None,
    }
}
