#![allow(clippy::arc_with_non_send_sync)]

mod compiler;
mod diagnostics;
mod execution;
mod frontend;
mod prepared;

use crate::executor::streaming::plan::PhysicalPlan;
use crate::executor::streaming::pool::SharedScheduler;
use crate::executor::streaming::query_registry::QueryRegistry;
use crate::executor::streaming::SessionTransactionController;
use crate::optimizer::OptimizerEngine;
use crate::planning::{ParameterizedQueryHandler, PlanCacheConfig, QueryPlanCache};
use crate::storage::QueryStorage;
use graphdb_core::metadata::index_manager::IndexMetadataManager;
use graphdb_core::metadata::SchemaManager;
use graphdb_core::StatsManager;
#[cfg(feature = "fulltext-search")]
use graphdb_fulltext::manager::FulltextIndexManager;
#[cfg(feature = "vector")]
use graphdb_sync::vector_sync::VectorSyncCoordinator;
use graphdb_sync::SyncManager;
use parking_lot::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub struct QueryPipelineManager<S: QueryStorage + 'static> {
    pub(crate) stats_manager: Arc<StatsManager>,
    pub(crate) optimizer_engine: Arc<OptimizerEngine>,
    pub(crate) plan_cache: Arc<QueryPlanCache>,
    pub(crate) param_handler: ParameterizedQueryHandler,
    pub(crate) schema_manager: Option<Arc<SchemaManager>>,
    pub(crate) index_manager: Option<Arc<dyn IndexMetadataManager>>,
    #[cfg(feature = "fulltext-search")]
    pub(crate) fulltext_manager: Option<Arc<FulltextIndexManager>>,
    #[cfg(feature = "vector")]
    pub(crate) vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pub(crate) storage: Option<Arc<RwLock<S>>>,
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) schema_generation: Arc<AtomicU64>,
    pub(crate) index_generation: Arc<AtomicU64>,
    pub(crate) query_registry: Option<Arc<QueryRegistry>>,
    /// Engine-level shared scheduler, created once and reused across queries.
    pub(crate) shared_scheduler: Option<Arc<SharedScheduler>>,
    pub(crate) session_controller: parking_lot::RwLock<Option<Arc<SessionTransactionController>>>,
    /// Serializes statistics collection to prevent concurrent re-collection.
    pub(crate) statistics_collect_lock: Arc<parking_lot::Mutex<()>>,
    /// Sample cap for per-tag/per-edge-type degree estimation during collection.
    pub(crate) statistics_sample_limit: usize,
    pub(crate) dml_shape_cache_enabled: bool,
    pub(crate) last_dml_plan: parking_lot::Mutex<Option<LastDmlPlan>>,
    pub last_dml_plan_hits: std::sync::atomic::AtomicU64,
    pub(crate) dml_template_ast:
        parking_lot::Mutex<Option<(String, Arc<crate::parser::ast::stmt::Ast>)>>,
    pub(crate) dml_template_ast_parse_count: std::sync::atomic::AtomicU64,
    pub(crate) dml_bind_skipped_count: std::sync::atomic::AtomicU64,
}

pub(crate) struct LastDmlPlan {
    pub normalized_text: String,
    pub space_name: Option<String>,
    pub schema_version: Option<u64>,
    pub param_sig: u64,
    pub plan: Arc<PhysicalPlan>,
}

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub fn with_optimizer(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
    ) -> Self {
        let plan_cache =
            Arc::new(QueryPlanCache::default().with_stats_manager(stats_manager.clone()));
        let param_handler = ParameterizedQueryHandler;

        optimizer_engine.set_cte_cache_stats_manager(stats_manager.clone());

        log::info!("Query pipeline manager created, using optimizer engine and query plan cache");

        Self {
            stats_manager,
            optimizer_engine,
            plan_cache,
            param_handler,
            schema_manager: None,
            index_manager: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(0)),
            index_generation: Arc::new(AtomicU64::new(0)),
            query_registry: None,
            shared_scheduler: None,
            session_controller: parking_lot::RwLock::new(None),
            statistics_collect_lock: Arc::new(parking_lot::Mutex::new(())),
            statistics_sample_limit: 10_000,
            dml_shape_cache_enabled: true,
            last_dml_plan: parking_lot::Mutex::new(None),
            last_dml_plan_hits: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast: parking_lot::Mutex::new(None),
            dml_template_ast_parse_count: std::sync::atomic::AtomicU64::new(0),
            dml_bind_skipped_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn with_statistics_sample_limit(mut self, sample_limit: usize) -> Self {
        self.statistics_sample_limit = sample_limit.max(1);
        self
    }

    /// Collect (or serve cached) statistics for a space into the optimizer's
    /// `StatisticsManager`.
    ///
    /// `force` bypasses the version gate so an explicit ANALYZE always
    /// refreshes the statistics. Collection is serialized internally.
    pub fn collect_statistics(
        &self,
        space: &str,
        force: bool,
    ) -> Result<crate::optimizer::stats::CollectedSummary, String> {
        let _guard = self.statistics_collect_lock.lock();
        let stats_manager = self.optimizer_engine.stats_manager().clone();
        let schema_version = self
            .schema_generation
            .load(std::sync::atomic::Ordering::Relaxed);
        if force {
            stats_manager.mark_space_dirty(space);
            self.optimizer_engine.invalidate_space_feedback(Some(space));
        }
        let storage: Arc<RwLock<dyn QueryStorage>> = self
            .storage
            .as_ref()
            .ok_or_else(|| "No storage binding available for statistics collection".to_string())?
            .clone();
        let data_epoch = storage.read().stats_epoch();
        let result = crate::optimizer::stats::StatisticsCollector::collect_space(
            &stats_manager,
            &storage,
            space,
            schema_version,
            data_epoch,
            self.statistics_sample_limit,
        );
        match &result {
            Ok(summary) => {
                log::info!(
                    "Statistics collection for space '{}': {} tags, {} edge types{}",
                    space,
                    summary.tags,
                    summary.edge_types,
                    if summary.cached { " (cached)" } else { "" }
                );
            }
            Err(error) => {
                log::warn!(
                    "Statistics collection failed for space '{}': {}",
                    space,
                    error
                );
            }
        }
        result
    }

    pub fn with_query_registry(mut self, registry: Arc<QueryRegistry>) -> Self {
        self.query_registry = Some(registry);
        self
    }

    pub fn with_shared_scheduler(mut self, scheduler: Arc<SharedScheduler>) -> Self {
        self.shared_scheduler = Some(scheduler);
        self
    }

    /// Set the shared scheduler (used when the pipeline is built before the
    /// scheduler is created, e.g. the vector-enabled server path).
    pub fn set_shared_scheduler(&mut self, scheduler: Option<Arc<SharedScheduler>>) {
        self.shared_scheduler = scheduler;
    }

    /// Set the query registry (used when the pipeline is built before the
    /// registry is created, e.g. the vector-enabled server path).
    pub fn set_query_registry(&mut self, registry: Option<Arc<QueryRegistry>>) {
        self.query_registry = registry;
    }

    /// Access the shared scheduler instance, if any.
    pub fn shared_scheduler(&self) -> Option<Arc<SharedScheduler>> {
        self.shared_scheduler.clone()
    }

    /// Access the query registry instance, if any.
    pub fn query_registry(&self) -> Option<Arc<QueryRegistry>> {
        self.query_registry.clone()
    }

    pub fn with_optimizer_and_cache(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
        plan_cache_config: PlanCacheConfig,
    ) -> Self {
        let plan_cache = Arc::new(
            QueryPlanCache::new(plan_cache_config).with_stats_manager(stats_manager.clone()),
        );
        let param_handler = ParameterizedQueryHandler;

        optimizer_engine.set_cte_cache_stats_manager(stats_manager.clone());

        log::info!(
            "Query pipeline manager created, using optimizer engine and custom query plan cache"
        );

        Self {
            stats_manager,
            optimizer_engine,
            plan_cache,
            param_handler,
            schema_manager: None,
            index_manager: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(0)),
            index_generation: Arc::new(AtomicU64::new(0)),
            query_registry: None,
            shared_scheduler: None,
            session_controller: parking_lot::RwLock::new(None),
            statistics_collect_lock: Arc::new(parking_lot::Mutex::new(())),
            statistics_sample_limit: 10_000,
            dml_shape_cache_enabled: true,
            last_dml_plan: parking_lot::Mutex::new(None),
            last_dml_plan_hits: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast: parking_lot::Mutex::new(None),
            dml_template_ast_parse_count: std::sync::atomic::AtomicU64::new(0),
            dml_bind_skipped_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn optimizer_engine(&self) -> &OptimizerEngine {
        &self.optimizer_engine
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn with_index_manager(mut self, index_manager: Arc<dyn IndexMetadataManager>) -> Self {
        self.index_manager = Some(index_manager);
        self
    }

    #[cfg(feature = "fulltext-search")]
    pub fn with_fulltext_manager(mut self, fulltext_manager: Arc<FulltextIndexManager>) -> Self {
        self.fulltext_manager = Some(fulltext_manager);
        self
    }

    #[cfg(feature = "vector")]
    pub fn with_vector_coordinator(
        mut self,
        vector_coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        self.vector_coordinator = Some(vector_coordinator);
        self
    }

    pub fn plan_cache(&self) -> &QueryPlanCache {
        &self.plan_cache
    }

    pub fn plan_cache_metrics(&self) -> Arc<crate::cache::PlanCacheStats> {
        self.plan_cache.stats()
    }

    pub fn clear_plan_cache(&self) {
        self.plan_cache.clear();
        log::info!("Query plan cache cleared");
    }

    pub fn with_sync_manager(mut self, sync_manager: Arc<SyncManager>) -> Self {
        self.sync_manager = Some(sync_manager);
        self
    }

    /// Toggle DML shape plan caching (independent switch / rollback path).
    pub fn with_dml_shape_cache(mut self, enabled: bool) -> Self {
        self.dml_shape_cache_enabled = enabled;
        self
    }

    /// Whether DML shape plan caching is enabled.
    pub fn dml_shape_cache_enabled(&self) -> bool {
        self.dml_shape_cache_enabled
    }

    pub fn dml_template_ast_parse_count(&self) -> u64 {
        self.dml_template_ast_parse_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dml_bind_skipped_count(&self) -> u64 {
        self.dml_bind_skipped_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The underlying storage binding, if any.
    pub fn storage(&self) -> Option<Arc<RwLock<S>>> {
        self.storage.clone()
    }
}
