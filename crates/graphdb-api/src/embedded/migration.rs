use crate::api_core::{CoreError, CoreResult};
use crate::embedded::database::GraphDatabase;
use crate::storage::GraphStorage;
use graphdb_migration::{MigrationConfig, MigrationPlan, MigrationReport};

impl GraphDatabase<GraphStorage> {
    pub fn generate_vertex_migration_plan(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> CoreResult<MigrationPlan> {
        let storage = self.storage();
        graphdb_migration::generate_vertex_plan(&*storage, space, tag, from_version, to_version)
            .map_err(|e| CoreError::Internal(e.to_string()))
    }

    pub fn generate_vertex_migration_plan_with_expand(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
        expand_contract: bool,
    ) -> CoreResult<MigrationPlan> {
        let storage = self.storage();
        graphdb_migration::generate_vertex_plan_with_expand(
            &*storage,
            space,
            tag,
            from_version,
            to_version,
            expand_contract,
        )
        .map_err(|e| CoreError::Internal(e.to_string()))
    }

    pub fn generate_edge_migration_plan(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> CoreResult<MigrationPlan> {
        let storage = self.storage();
        graphdb_migration::generate_edge_plan(&*storage, space, edge_type, from_version, to_version)
            .map_err(|e| CoreError::Internal(e.to_string()))
    }

    pub fn generate_edge_migration_plan_with_expand(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
        expand_contract: bool,
    ) -> CoreResult<MigrationPlan> {
        let storage = self.storage();
        graphdb_migration::generate_edge_plan_with_expand(
            &*storage,
            space,
            edge_type,
            from_version,
            to_version,
            expand_contract,
        )
        .map_err(|e| CoreError::Internal(e.to_string()))
    }

    pub fn execute_migration_plan(&self, plan: &MigrationPlan) -> CoreResult<MigrationReport> {
        let mut storage = self.storage_mut();
        let start = std::time::Instant::now();
        self.stats_manager().record_migration_start();
        let res = graphdb_migration::execute_migration_plan(&mut *storage, plan)
            .map_err(|e| CoreError::Internal(e.to_string()));
        let elapsed = start.elapsed().as_millis() as u64;
        match &res {
            Ok(report) if report.success => {
                self.stats_manager()
                    .record_migration_success(report.rows_migrated, elapsed);
            }
            Ok(_) | Err(_) => {
                self.stats_manager().record_migration_failure(elapsed);
            }
        }
        res
    }

    pub fn execute_migration_plan_with_config(
        &self,
        plan: &MigrationPlan,
        config: &MigrationConfig,
    ) -> CoreResult<MigrationReport> {
        let mut storage = self.storage_mut();
        let start = std::time::Instant::now();
        self.stats_manager().record_migration_start();
        let res =
            graphdb_migration::execute_migration_plan_with_config(&mut *storage, plan, config)
                .map_err(|e| CoreError::Internal(e.to_string()));
        let elapsed = start.elapsed().as_millis() as u64;
        match &res {
            Ok(report) if report.success => {
                self.stats_manager()
                    .record_migration_success(report.rows_migrated, elapsed);
            }
            Ok(_) | Err(_) => {
                self.stats_manager().record_migration_failure(elapsed);
            }
        }
        res
    }

    pub fn rollback_migration(&self, plan: &MigrationPlan) -> CoreResult<MigrationReport> {
        let mut storage = self.storage_mut();
        graphdb_migration::rollback_migration(&mut *storage, plan)
            .map_err(|e| CoreError::Internal(e.to_string()))
    }

    pub fn migration_metrics(&self) -> std::collections::HashMap<graphdb_metrics::MetricType, u64> {
        self.stats_manager().get_migration_metrics()
    }
}
