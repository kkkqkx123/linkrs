use parking_lot::RwLock;
use std::sync::Arc;

use graphdb_migration::{execute_migration_plan, generate_edge_plan, generate_vertex_plan};

use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::{StorageClient, StorageReader};

#[derive(Debug, Clone)]
pub struct MigrationCmdInfo {
    pub space_name: String,
    pub label_name: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
}

impl MigrationCmdInfo {
    pub fn new(space_name: String, label_name: String) -> Self {
        Self {
            space_name,
            label_name,
            is_edge: false,
            from_version: 1,
            to_version: 1,
        }
    }

    pub fn for_edge(mut self) -> Self {
        self.is_edge = true;
        self
    }

    pub fn with_versions(mut self, from: u64, to: u64) -> Self {
        self.from_version = from;
        self.to_version = to;
        self
    }
}

#[derive(Debug)]
pub struct MigrateExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    cmd_info: MigrationCmdInfo,
}

impl<S: StorageClient> MigrateExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        cmd_info: MigrationCmdInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "MigrateExecutor".to_string(), storage, expr_context),
            cmd_info,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for MigrateExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage().clone();

        // Phase 1: Generate plan with read lock
        let plan = {
            let reader = storage.read();
            let reader_ref: &dyn StorageReader = &*reader;

            let result = if self.cmd_info.is_edge {
                generate_edge_plan(
                    reader_ref,
                    &self.cmd_info.space_name,
                    &self.cmd_info.label_name,
                    self.cmd_info.from_version,
                    self.cmd_info.to_version,
                )
            } else {
                generate_vertex_plan(
                    reader_ref,
                    &self.cmd_info.space_name,
                    &self.cmd_info.label_name,
                    self.cmd_info.from_version,
                    self.cmd_info.to_version,
                )
            };

            match result {
                Ok(plan) => plan,
                Err(e) => {
                    return Ok(ExecutionResult::Error(format!("Failed to generate plan: {}", e)));
                }
            }
        };

        if plan.is_empty() {
            return Ok(ExecutionResult::Success);
        }

        // Phase 2: Execute with write lock (StorageClient provides both read + write)
        {
            let mut writer = storage.write();
            let storage_client: &mut dyn StorageClient = &mut *writer;

            match execute_migration_plan(storage_client, &plan) {
                Ok(report) => {
                    let mut summary = plan.print_summary();
                    summary.push_str("\n---\n");
                    summary.push_str(&report.print_summary());
                    log::info!("Migration completed: {}", summary);
                }
                Err(e) => {
                    return Ok(ExecutionResult::Error(format!(
                        "Migration execution failed: {}",
                        e
                    )));
                }
            }
        }

        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> crate::query::executor::base::DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> crate::query::executor::base::DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "MigrateExecutor"
    }

    fn description(&self) -> &str {
        "Migrate existing data to match schema changes"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for MigrateExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
