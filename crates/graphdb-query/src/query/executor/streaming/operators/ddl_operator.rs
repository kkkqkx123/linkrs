use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::{NullType, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::spec::MigrateAction;
use crate::query::executor::streaming::slot::{SlotInfo, SlotLayout};
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, IndexManageNode, SpaceManageNode, TagManageNode, UserManageNode,
};
use crate::storage::{QueryStorage, StorageSchemaOps};

/// Pre-computed layout for DDL manage result chunks (action, name, status).
fn manage_result_layout() -> Arc<SlotLayout> {
    use std::sync::OnceLock;
    static LAYOUT: OnceLock<Arc<SlotLayout>> = OnceLock::new();
    LAYOUT
        .get_or_init(|| {
            Arc::new(SlotLayout::new(vec![
                SlotInfo {
                    slot_id: 0,
                    name: "action".to_string(),
                    alias: None,
                    data_type: Some(crate::core::DataType::String),
                    nullable: false,
                    origin: None,
                },
                SlotInfo {
                    slot_id: 1,
                    name: "name".to_string(),
                    alias: None,
                    data_type: Some(crate::core::DataType::String),
                    nullable: true,
                    origin: None,
                },
                SlotInfo {
                    slot_id: 2,
                    name: "status".to_string(),
                    alias: None,
                    data_type: Some(crate::core::DataType::String),
                    nullable: false,
                    origin: None,
                },
            ]))
        })
        .clone()
}

mod auth_executor;
mod maintenance_executor;
mod schema_executor;

fn make_manage_result(action: &str, name: Option<&str>, status: &str) -> DataChunk {
    let name_val = name
        .map(Value::string)
        .unwrap_or(Value::Null(NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![Value::string(action), name_val, Value::string(status)]],
        manage_result_layout(),
    )
}

fn exec_ddl<F>(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
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
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    f: F,
) -> Result<Option<DataChunk>, QueryError>
where
    F: FnOnce(&mut dyn QueryStorage) -> Result<(), QueryError>,
{
    if let Some(lock) = storage {
        let mut writer = lock.write();
        f(&mut *writer).map(|_| Some(make_manage_result("auth", None, "executed")))
    } else {
        Ok(Some(make_manage_result("auth", None, "no-storage")))
    }
}

fn get_reader(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
) -> Result<parking_lot::RwLockReadGuard<'_, dyn QueryStorage>, QueryError> {
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
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: SpaceManageNode,
        emitted: bool,
    },
    TagManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: TagManageNode,
        emitted: bool,
    },
    EdgeManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: EdgeManageNode,
        emitted: bool,
    },
    IndexManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: IndexManageNode,
        emitted: bool,
    },
    DeleteIndex {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        index_name: String,
        emitted: bool,
    },
    UserManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: UserManageNode,
        emitted: bool,
    },
    ShowStats {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    ShowConfigs {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    ShowQueries {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    ShowSessions {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    Analyze {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        analyze_target: String,
        target_name: Option<String>,
        emitted: bool,
    },
    Migrate {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        action: MigrateAction,
        migration_data: Option<String>,
        emitted: bool,
    },
}

impl DdlOperator {
    pub fn from_spec(
        spec: &super::spec::DdlSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    ) -> Self {
        match spec {
            super::spec::DdlSpec::SpaceManage { command } => DdlOperator::SpaceManage {
                storage: storage.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::TagManage {
                space_name,
                command,
            } => DdlOperator::TagManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::EdgeManage {
                space_name,
                command,
            } => DdlOperator::EdgeManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::IndexManage {
                space_name,
                command,
            } => DdlOperator::IndexManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::DeleteIndex {
                space_name,
                index_name,
            } => DdlOperator::DeleteIndex {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::UserManage { command } => DdlOperator::UserManage {
                storage: storage.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowStats { space_name } => DdlOperator::ShowStats {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowConfigs { space_name } => DdlOperator::ShowConfigs {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowQueries { space_name } => DdlOperator::ShowQueries {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowSessions { space_name } => DdlOperator::ShowSessions {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::Analyze { space_name } => DdlOperator::Analyze {
                storage: storage.clone(),
                space_name: space_name.clone(),
                analyze_target: String::new(),
                target_name: None,
                emitted: false,
            },
            super::spec::DdlSpec::Migrate {
                space_name,
                action,
                migration_data,
            } => DdlOperator::Migrate {
                storage,
                space_name: space_name.clone(),
                action: *action,
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
            | DdlOperator::ShowConfigs { .. }
            | DdlOperator::ShowQueries { .. }
            | DdlOperator::ShowSessions { .. }
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
            } => schema_executor::execute_space_manage(storage, command, emitted, base),
            DdlOperator::TagManage {
                storage,
                space_name,
                command,
                emitted,
            } => schema_executor::execute_tag_manage(storage, space_name, command, emitted, base),
            DdlOperator::EdgeManage {
                storage,
                space_name,
                command,
                emitted,
            } => schema_executor::execute_edge_manage(storage, space_name, command, emitted, base),
            DdlOperator::IndexManage {
                storage,
                space_name,
                command,
                emitted,
            } => schema_executor::execute_index_manage(storage, space_name, command, emitted, base),
            DdlOperator::DeleteIndex {
                storage,
                space_name,
                index_name,
                emitted,
            } => schema_executor::execute_delete_index(
                storage, space_name, index_name, emitted, base,
            ),
            DdlOperator::UserManage {
                storage,
                command,
                emitted,
            } => auth_executor::execute_user_manage(storage, command, emitted, base),
            DdlOperator::ShowStats {
                storage, emitted, ..
            } => maintenance_executor::execute_show_stats(storage, emitted, base),
            DdlOperator::ShowConfigs {
                storage,
                space_name,
                emitted,
            } => maintenance_executor::execute_show_configs(storage, space_name, emitted, base),
            DdlOperator::ShowQueries {
                storage,
                space_name,
                emitted,
            } => maintenance_executor::execute_show_queries(storage, space_name, emitted, base),
            DdlOperator::ShowSessions {
                storage,
                space_name,
                emitted,
            } => maintenance_executor::execute_show_sessions(storage, space_name, emitted, base),
            DdlOperator::Analyze {
                storage,
                space_name,
                analyze_target,
                target_name,
                emitted,
            } => maintenance_executor::execute_analyze(
                storage,
                space_name,
                analyze_target,
                target_name,
                emitted,
                base,
            ),
            DdlOperator::Migrate {
                storage,
                space_name,
                action,
                migration_data: _,
                emitted,
            } => maintenance_executor::execute_migrate(storage, space_name, action, emitted, base),
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            DdlOperator::SpaceManage { .. }
            | DdlOperator::TagManage { .. }
            | DdlOperator::EdgeManage { .. }
            | DdlOperator::IndexManage { .. }
            | DdlOperator::DeleteIndex { .. }
            | DdlOperator::UserManage { .. }
            | DdlOperator::ShowStats { .. }
            | DdlOperator::ShowConfigs { .. }
            | DdlOperator::ShowQueries { .. }
            | DdlOperator::ShowSessions { .. }
            | DdlOperator::Analyze { .. }
            | DdlOperator::Migrate { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_stopped();
                }
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            DdlOperator::SpaceManage { .. }
            | DdlOperator::TagManage { .. }
            | DdlOperator::EdgeManage { .. }
            | DdlOperator::IndexManage { .. }
            | DdlOperator::DeleteIndex { .. }
            | DdlOperator::UserManage { .. }
            | DdlOperator::ShowStats { .. }
            | DdlOperator::ShowConfigs { .. }
            | DdlOperator::ShowQueries { .. }
            | DdlOperator::ShowSessions { .. }
            | DdlOperator::Analyze { .. }
            | DdlOperator::Migrate { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}
