use std::sync::Arc;

use parking_lot::RwLock;

use graphdb_core::error::QueryError;
use graphdb_core::{NullType, Value};
use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::{
    EdgeManageCommand, IndexManageCommand, MigrateAction, SpaceManageCommand, TagManageCommand,
    UserManageCommand,
};
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::{SlotInfo, SlotLayout};
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
                    data_type: Some(graphdb_core::DataType::String),
                    nullable: false,
                    origin: None,
                },
                SlotInfo {
                    slot_id: 1,
                    name: "name".to_string(),
                    alias: None,
                    data_type: Some(graphdb_core::DataType::String),
                    nullable: true,
                    origin: None,
                },
                SlotInfo {
                    slot_id: 2,
                    name: "status".to_string(),
                    alias: None,
                    data_type: Some(graphdb_core::DataType::String),
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
pub enum DdlOperatorKind {
    SpaceManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: SpaceManageCommand,
        emitted: bool,
    },
    TagManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: TagManageCommand,
        emitted: bool,
    },
    EdgeManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: EdgeManageCommand,
        emitted: bool,
    },
    IndexManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: IndexManageCommand,
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
        command: UserManageCommand,
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

/// DDL operator.
///
/// Wraps [`DdlOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct DdlOperator {
    pub kind: DdlOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl DdlOperator {
    pub fn from_spec(
        spec: &super::spec::DdlSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::DdlSpec::SpaceManage { command } => DdlOperatorKind::SpaceManage {
                storage: storage.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::TagManage {
                space_name,
                command,
            } => DdlOperatorKind::TagManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::EdgeManage {
                space_name,
                command,
            } => DdlOperatorKind::EdgeManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::IndexManage {
                space_name,
                command,
            } => DdlOperatorKind::IndexManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::DeleteIndex {
                space_name,
                index_name,
            } => DdlOperatorKind::DeleteIndex {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::UserManage { command } => DdlOperatorKind::UserManage {
                storage: storage.clone(),
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowStats { space_name } => DdlOperatorKind::ShowStats {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowConfigs { space_name } => DdlOperatorKind::ShowConfigs {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowQueries { space_name } => DdlOperatorKind::ShowQueries {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowSessions { space_name } => DdlOperatorKind::ShowSessions {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::Analyze { space_name } => DdlOperatorKind::Analyze {
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
            } => DdlOperatorKind::Migrate {
                storage,
                space_name: space_name.clone(),
                action: *action,
                migration_data: migration_data.clone(),
                emitted: false,
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: DdlOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        input.open()?;
        Ok(())
    }

    pub fn next(
        &mut self,
        _input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
            DdlOperatorKind::SpaceManage { .. } => schema_executor::execute_space_manage(self),
            DdlOperatorKind::TagManage { .. } => schema_executor::execute_tag_manage(self),
            DdlOperatorKind::EdgeManage { .. } => schema_executor::execute_edge_manage(self),
            DdlOperatorKind::IndexManage { .. } => schema_executor::execute_index_manage(self),
            DdlOperatorKind::DeleteIndex { .. } => schema_executor::execute_delete_index(self),
            DdlOperatorKind::UserManage { .. } => auth_executor::execute_user_manage(self),
            DdlOperatorKind::ShowStats { .. } => maintenance_executor::execute_show_stats(self),
            DdlOperatorKind::ShowConfigs { .. } => maintenance_executor::execute_show_configs(self),
            DdlOperatorKind::ShowQueries { .. } => maintenance_executor::execute_show_queries(self),
            DdlOperatorKind::ShowSessions { .. } => {
                maintenance_executor::execute_show_sessions(self)
            }
            DdlOperatorKind::Analyze { .. } => maintenance_executor::execute_analyze(self),
            DdlOperatorKind::Migrate { .. } => maintenance_executor::execute_migrate(self),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}
