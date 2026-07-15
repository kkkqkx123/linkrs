#![allow(clippy::arc_with_non_send_sync)]

mod compiler;
mod diagnostics;
mod execution;
mod frontend;
mod metadata;

use crate::core::metadata::index_manager::IndexMetadataManager;
use crate::core::metadata::SchemaManager;
use crate::core::StatsManager;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::query::optimizer::OptimizerEngine;
use crate::query::planning::{ParameterizedQueryHandler, PlanCacheConfig, QueryPlanCache};
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::QueryStorage;
#[cfg(feature = "qdrant")]
use crate::sync::vector_sync::VectorSyncCoordinator;
use crate::sync::SyncManager;
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
    #[cfg(feature = "qdrant")]
    pub(crate) vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pub(crate) storage: Option<Arc<RwLock<S>>>,
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) schema_generation: Arc<AtomicU64>,
    pub(crate) query_registry: Option<Arc<QueryRegistry>>,
}

fn next_transaction_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TXN_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_TXN_ID.fetch_add(1, Ordering::Relaxed)
}

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub fn replace_storage(&mut self, storage: Arc<RwLock<S>>) {
        self.storage = Some(storage);
    }

    pub fn with_optimizer(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
    ) -> Self {
        let plan_cache =
            Arc::new(QueryPlanCache::default().with_stats_manager(stats_manager.clone()));
        let param_handler = ParameterizedQueryHandler::default();

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
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(0)),
            query_registry: None,
        }
    }

    pub fn with_query_registry(mut self, registry: Arc<QueryRegistry>) -> Self {
        self.query_registry = Some(registry);
        self
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
        let param_handler = ParameterizedQueryHandler::default();

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
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(0)),
            query_registry: None,
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

    #[cfg(feature = "qdrant")]
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

    pub fn plan_cache_metrics(&self) -> Arc<crate::query::cache::PlanCacheStats> {
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
}
