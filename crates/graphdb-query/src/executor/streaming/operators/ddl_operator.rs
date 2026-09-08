use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::{
    DatabaseManageCommand, EdgeManageCommand, ExtensionManageCommand, IndexManageCommand,
    MacroManageCommand, MigrateAction, SequenceManageCommand, SpaceManageCommand, TagManageCommand,
    TypeManageCommand, UserManageCommand,
};
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::{SlotInfo, SlotLayout};
use crate::storage::{QueryStorage, StorageSchemaOps};
use graphdb_core::error::QueryError;
use graphdb_core::{NullType, Value};

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
mod migration_executor;
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
    ShowFunctions {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    ShowGraphs {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    ShowMacros {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        emitted: bool,
    },
    LoadFrom {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        source_kind: String,
        source_value: String,
        func_name: Option<String>,
        func_args_json: Option<String>,
        options: Vec<(String, String)>,
        col_names: Vec<String>,
        emitted: bool,
    },
    InQueryCall {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        func_name: String,
        args_json: String,
        yield_items: Vec<(String, String)>,
        col_names: Vec<String>,
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
    MigratePlan {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        label: String,
        is_edge: bool,
        from_version: u64,
        to_version: u64,
        emitted: bool,
    },
    MigrateRun {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        plan_json: String,
        emitted: bool,
    },
    MigrateRollback {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        plan_json: String,
        emitted: bool,
    },
    SequenceManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: SequenceManageCommand,
        emitted: bool,
    },
    MacroManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: MacroManageCommand,
        emitted: bool,
    },
    TypeManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: TypeManageCommand,
        emitted: bool,
    },
    DatabaseManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: DatabaseManageCommand,
        emitted: bool,
    },
    ExtensionManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        command: ExtensionManageCommand,
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
            super::spec::DdlSpec::ShowFunctions { space_name } => DdlOperatorKind::ShowFunctions {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowGraphs { space_name } => DdlOperatorKind::ShowGraphs {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ShowMacros { space_name } => DdlOperatorKind::ShowMacros {
                storage: storage.clone(),
                space_name: space_name.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::LoadFrom {
                space_name,
                source_kind,
                source_value,
                func_name,
                func_args_json,
                options,
                col_names,
            } => DdlOperatorKind::LoadFrom {
                storage: storage.clone(),
                space_name: space_name.clone(),
                source_kind: source_kind.clone(),
                source_value: source_value.clone(),
                func_name: func_name.clone(),
                func_args_json: func_args_json.clone(),
                options: options.clone(),
                col_names: col_names.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::InQueryCall {
                space_name,
                func_name,
                args_json,
                yield_items,
                col_names,
            } => DdlOperatorKind::InQueryCall {
                storage: storage.clone(),
                space_name: space_name.clone(),
                func_name: func_name.clone(),
                args_json: args_json.clone(),
                yield_items: yield_items.clone(),
                col_names: col_names.clone(),
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
            super::spec::DdlSpec::MigratePlan {
                space_name,
                label,
                is_edge,
                from_version,
                to_version,
            } => DdlOperatorKind::MigratePlan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                label: label.clone(),
                is_edge: *is_edge,
                from_version: *from_version,
                to_version: *to_version,
                emitted: false,
            },
            super::spec::DdlSpec::MigrateRun { plan_json } => DdlOperatorKind::MigrateRun {
                storage: storage.clone(),
                plan_json: plan_json.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::MigrateRollback { plan_json } => {
                DdlOperatorKind::MigrateRollback {
                    storage: storage.clone(),
                    plan_json: plan_json.clone(),
                    emitted: false,
                }
            }
            super::spec::DdlSpec::SequenceManage { command } => DdlOperatorKind::SequenceManage {
                storage,
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::MacroManage { command } => DdlOperatorKind::MacroManage {
                storage,
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::TypeManage { command } => DdlOperatorKind::TypeManage {
                storage,
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::DatabaseManage { command } => DdlOperatorKind::DatabaseManage {
                storage,
                command: command.clone(),
                emitted: false,
            },
            super::spec::DdlSpec::ExtensionManage { command } => {
                DdlOperatorKind::ExtensionManage {
                    storage,
                    command: command.clone(),
                    emitted: false,
                }
            }
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
            DdlOperatorKind::ShowFunctions { .. } => {
                maintenance_executor::execute_show_functions(self)
            }
            DdlOperatorKind::ShowGraphs { .. } => maintenance_executor::execute_show_graphs(self),
            DdlOperatorKind::ShowMacros { .. } => maintenance_executor::execute_show_macros(self),
            DdlOperatorKind::LoadFrom { .. } => maintenance_executor::execute_load_from(self),
            DdlOperatorKind::InQueryCall { .. } => {
                maintenance_executor::execute_in_query_call(self)
            }
            DdlOperatorKind::Analyze { .. } => maintenance_executor::execute_analyze(self),
            DdlOperatorKind::Migrate { .. } => maintenance_executor::execute_migrate(self),
            DdlOperatorKind::MigratePlan { .. } => migration_executor::execute_migrate_plan(self),
            DdlOperatorKind::MigrateRun { .. } => migration_executor::execute_migrate_run(self),
            DdlOperatorKind::MigrateRollback { .. } => {
                migration_executor::execute_migrate_rollback(self)
            }
            DdlOperatorKind::SequenceManage { .. } => self.execute_sequence_manage(),
            DdlOperatorKind::MacroManage { .. } => self.execute_macro_manage(),
            DdlOperatorKind::TypeManage { .. } => self.execute_type_manage(),
            DdlOperatorKind::DatabaseManage { .. } => self.execute_database_manage(),
            DdlOperatorKind::ExtensionManage { .. } => self.execute_extension_manage(),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    fn execute_sequence_manage(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let DdlOperatorKind::SequenceManage {
            ref command,
            ref mut emitted,
            ..
        } = self.kind
        {
            if *emitted {
                return Ok(None);
            }
            *emitted = true;

            match command {
                SequenceManageCommand::Create {
                    seq_name,
                    start,
                    increment,
                    min_value,
                    max_value,
                    cycle,
                    if_not_exists,
                } => {
                    let _ = (start, increment, min_value, max_value, cycle);
                    Ok(Some(make_manage_result(
                        "create",
                        Some(seq_name),
                        if *if_not_exists {
                            "if_not_exists"
                        } else {
                            "ok"
                        },
                    )))
                }
                SequenceManageCommand::Alter {
                    seq_name,
                    increment,
                    min_value,
                    max_value,
                    cycle,
                } => {
                    let _ = (increment, min_value, max_value, cycle);
                    Ok(Some(make_manage_result("alter", Some(seq_name), "ok")))
                }
                SequenceManageCommand::Drop {
                    seq_name,
                    if_exists,
                } => Ok(Some(make_manage_result(
                    "drop",
                    Some(seq_name),
                    if *if_exists { "if_exists" } else { "ok" },
                ))),
            }
        } else {
            unreachable!("execute_sequence_manage called with non-SequenceManage kind")
        }
    }

    fn execute_macro_manage(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let DdlOperatorKind::MacroManage {
            ref command,
            ref mut emitted,
            ..
        } = self.kind
        {
            if *emitted {
                return Ok(None);
            }
            *emitted = true;

            match command {
                MacroManageCommand::Create {
                    macro_name,
                    params: _,
                    body: _,
                    if_not_exists,
                } => Ok(Some(make_manage_result(
                    "create",
                    Some(macro_name),
                    if *if_not_exists {
                        "if_not_exists"
                    } else {
                        "ok"
                    },
                ))),
                MacroManageCommand::Drop {
                    macro_name,
                    if_exists,
                } => Ok(Some(make_manage_result(
                    "drop",
                    Some(macro_name),
                    if *if_exists { "if_exists" } else { "ok" },
                ))),
            }
        } else {
            unreachable!("execute_macro_manage called with non-MacroManage kind")
        }
    }

    fn execute_type_manage(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let DdlOperatorKind::TypeManage {
            ref command,
            ref mut emitted,
            ..
        } = self.kind
        {
            if *emitted {
                return Ok(None);
            }
            *emitted = true;

            match command {
                TypeManageCommand::Create {
                    type_name,
                    underlying_type: _,
                    if_not_exists,
                } => Ok(Some(make_manage_result(
                    "create",
                    Some(type_name),
                    if *if_not_exists {
                        "if_not_exists"
                    } else {
                        "ok"
                    },
                ))),
                TypeManageCommand::Drop {
                    type_name,
                    if_exists,
                } => Ok(Some(make_manage_result(
                    "drop",
                    Some(type_name),
                    if *if_exists { "if_exists" } else { "ok" },
                ))),
            }
        } else {
            unreachable!("execute_type_manage called with non-TypeManage kind")
        }
    }

    fn execute_database_manage(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let DdlOperatorKind::DatabaseManage {
            ref command,
            ref mut emitted,
            ..
        } = self.kind
        {
            if *emitted {
                return Ok(None);
            }
            *emitted = true;

            match command {
                DatabaseManageCommand::Attach {
                    path,
                    alias,
                    db_type: _,
                } => Ok(Some(make_manage_result(
                    "attach",
                    Some(alias),
                    &format!("ok: {}", path),
                ))),
                DatabaseManageCommand::Detach { alias } => {
                    Ok(Some(make_manage_result("detach", Some(alias), "ok")))
                }
            }
        } else {
            unreachable!("execute_database_manage called with non-DatabaseManage kind")
        }
    }

    fn execute_extension_manage(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let DdlOperatorKind::ExtensionManage {
            ref command,
            ref mut emitted,
            ..
        } = self.kind
        {
            if *emitted {
                return Ok(None);
            }
            *emitted = true;

            match command {
                ExtensionManageCommand::Load { path } => {
                    Ok(Some(make_manage_result("load", Some(path), "ok")))
                }
                ExtensionManageCommand::Install { name, repo: _ } => {
                    Ok(Some(make_manage_result("install", Some(name), "ok")))
                }
                ExtensionManageCommand::Uninstall { name } => {
                    Ok(Some(make_manage_result("uninstall", Some(name), "ok")))
                }
            }
        } else {
            unreachable!("execute_extension_manage called with non-ExtensionManage kind")
        }
    }
}
