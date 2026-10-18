//! Builder / configuration surface for [`QueryApi`].
//!
//! The ~20 `with_*` constructors and accessor/configuration methods were split
//! out of `query_api.rs` so the execution core stays readable. They all return
//! `Self` (builder chaining) or expose a pipeline setting; pipeline internals
//! (the `pipeline_manager` field) are reached through the parent module.

use crate::api_core::cost_profile::cost_config_for_profile;
use crate::api_core::cost_profile::resolve_space_cost_configs;
use crate::api_core::error::{CoreError, CoreResult};
use crate::api_core::QueryApi;
use linkrs_config::{RuntimeConfig, StorageCostProfile};
use linkrs_core::metadata::SchemaManager;
use linkrs_metrics::StatsManager;
use linkrs_query::executor::streaming::pool::SharedScheduler;
use linkrs_query::executor::streaming::query_registry::QueryRegistry;
use linkrs_query::extensions::ExtensionRegistry;
use linkrs_query::optimizer::CostModelConfig;
use linkrs_query::query_manager::QueryManager;
use linkrs_query::OptimizerEngine;
use linkrs_storage::StorageClient;
#[cfg(feature = "vector")]
use linkrs_sync::backend::VectorBackend;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

impl<S: StorageClient + Clone + 'static> QueryApi<S> {
    /// Attach a schema manager to the pipeline.
    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.pipeline_manager = self.pipeline_manager.with_schema_manager(schema_manager);
        self
    }

    /// Attach a sync manager to the pipeline.
    pub fn with_sync_manager(mut self, sync_manager: Arc<linkrs_sync::SyncManager>) -> Self {
        self.pipeline_manager = self.pipeline_manager.with_sync_manager(sync_manager);
        self
    }

    /// Create a new QueryApi using an externally configured optimizer engine,
    /// so server-level settings (e.g. the `[parallel]` partitioning section)
    /// reach the query pipeline. The engine instance is shared by value.
    pub fn with_optimizer_engine(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
        schema_manager: Option<Arc<SchemaManager>>,
    ) -> Self {
        let pipeline = Self::pipeline_with_engine(storage, stats_manager, optimizer_engine);
        let pipeline = match schema_manager {
            Some(sm) => pipeline.with_schema_manager(sm),
            None => pipeline,
        };
        Self {
            pipeline_manager: pipeline,
        }
    }

    /// Apply an explicit cost model configuration, replacing the default
    /// engine built by the constructors.
    ///
    /// Assembly-time only: call before the first query. The replacement
    /// engine is freshly built, exactly like the constructor default;
    /// previously installed per-space overrides are carried over.
    pub fn with_cost_config(mut self, config: CostModelConfig) -> Self {
        let spaces = self
            .pipeline_manager
            .optimizer_engine()
            .space_cost_configs();
        let engine = OptimizerEngine::new(config);
        engine.set_space_cost_configs(spaces);
        self.pipeline_manager.set_optimizer_engine(Arc::new(engine));
        self
    }

    /// Apply a storage cost profile with runtime knowledge.
    ///
    /// Memory-mode deployments short-circuit to the in-memory preset without
    /// touching the filesystem; embedded in-memory users must call this (or
    /// [`Self::with_cost_config`]), otherwise the constructor default prices
    /// queries with mechanical-drive costs.
    pub fn with_cost_profile(
        mut self,
        profile: StorageCostProfile,
        runtime: &RuntimeConfig,
    ) -> Self {
        let resolved = profile.resolve_for_runtime(runtime);
        self = self.with_cost_config(cost_config_for_profile(resolved));
        self.pipeline_manager
            .set_default_cost_profile_label(format!("{resolved:?}"));
        self
    }

    /// Apply per-space storage cost profile overrides.
    ///
    /// Spaces absent from the map keep the global profile; memory-mode
    /// deployments short-circuit every space to the in-memory preset.
    /// Assembly-time only: call before the first query.
    pub fn with_space_cost_profiles(
        mut self,
        profiles: &HashMap<String, StorageCostProfile>,
        runtime: &RuntimeConfig,
    ) -> Self {
        let (configs, labels) = resolve_space_cost_configs(profiles, runtime);
        self.pipeline_manager
            .set_space_cost_configs(configs, labels);
        self
    }

    /// Override the global cost profile display label (assembly-time only).
    ///
    /// Used by assemblies that build their own engine (e.g. the network
    /// server with extra engine settings) so EXPLAIN still names the
    /// effective profile.
    pub fn with_default_cost_profile_label(mut self, label: String) -> Self {
        self.pipeline_manager.set_default_cost_profile_label(label);
        self
    }

    /// Effective cost model configuration currently in force.
    pub fn cost_config(&self) -> CostModelConfig {
        *self.pipeline_manager.optimizer_engine().cost_config()
    }

    /// Apply the server-configured statistics collection settings to the
    /// pipeline (sampling window and automatic-refresh epoch delta).
    ///
    /// The pipeline keeps fallback defaults when this is never called, so
    /// embedded users without server config are unaffected.
    pub fn with_statistics_settings(mut self, sample_limit: usize, min_epoch_delta: u64) -> Self {
        self.pipeline_manager = self
            .pipeline_manager
            .with_statistics_sample_limit(sample_limit)
            .with_statistics_min_epoch_delta(min_epoch_delta);
        self
    }

    /// Effective statistics sampling window currently in force.
    pub fn statistics_sample_limit(&self) -> usize {
        self.pipeline_manager.statistics_sample_limit()
    }

    /// Set the retained password-history depth applied by user-management
    /// operations (assembly-time only).
    pub fn with_password_history_depth(mut self, depth: usize) -> Self {
        self.pipeline_manager = self.pipeline_manager.with_password_history_depth(depth);
        self
    }

    /// Effective statistics refresh epoch delta currently in force.
    pub fn statistics_min_epoch_delta(&self) -> u64 {
        self.pipeline_manager.statistics_min_epoch_delta()
    }

    /// Install a shared scheduler and query registry into an existing
    /// pipeline (used when the pipeline was built by another constructor).
    pub fn install_shared_scheduler(
        &mut self,
        shared_scheduler: Arc<SharedScheduler>,
        query_registry: Arc<QueryRegistry>,
    ) {
        self.pipeline_manager
            .set_shared_scheduler(Some(shared_scheduler));
        self.pipeline_manager
            .set_query_registry(Some(query_registry));
    }

    /// Access the shared scheduler instance held by the pipeline, if any.
    pub fn shared_scheduler(&self) -> Option<Arc<SharedScheduler>> {
        self.pipeline_manager.shared_scheduler()
    }

    /// Access the query registry instance held by the pipeline, if any.
    pub fn query_registry(&self) -> Option<Arc<QueryRegistry>> {
        self.pipeline_manager.query_registry()
    }

    /// Install the global QueryManager so per-query runtimes forward KILL QUERY,
    /// finish tracking, and progress events into one registry. Mirror of
    /// [`install_shared_scheduler`]: called once at server startup.
    pub fn install_query_manager(&mut self, query_manager: Arc<QueryManager>) {
        self.pipeline_manager.set_query_manager(Some(query_manager));
    }

    /// Install a shared pipeline-extension registry (unstable experimental
    /// API) into an existing pipeline.
    pub fn set_extensions(&mut self, extensions: Arc<ExtensionRegistry>) {
        self.pipeline_manager.set_extensions(extensions);
    }

    /// Pipeline-extension registry backing this API.
    pub fn extensions(&self) -> Arc<ExtensionRegistry> {
        self.pipeline_manager.extensions()
    }

    /// Collect (or serve cached) optimizer statistics for a space.
    ///
    /// `force` bypasses the schema-version gate so an explicit ANALYZE always
    /// refreshes the statistics. Failures are returned as `Err`, never panic.
    pub fn collect_statistics(&self, space: &str, force: bool) -> CoreResult<()> {
        self.pipeline_manager
            .collect_statistics(space, force)
            .map_err(CoreError::Internal)
            .map(|_| ())
    }

    /// Query-plan-cache hit rate across all executed statements.
    pub fn plan_cache_hit_rate(&self) -> f64 {
        self.pipeline_manager.plan_cache_metrics().hit_rate()
    }

    /// Number of entries currently held in the query plan cache.
    pub fn plan_cache_len(&self) -> usize {
        self.pipeline_manager.plan_cache().len()
    }

    /// Create a new QueryApi instance with an existing shared VectorBackend
    #[cfg(feature = "vector")]
    pub async fn with_vector_backend(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        backend: VectorBackend,
        schema_manager: Option<Arc<SchemaManager>>,
    ) -> CoreResult<Self> {
        let optimizer_engine = Arc::new(OptimizerEngine::default());

        // Create a VectorSyncCoordinator with the shared backend (no embedding service for query-only use)
        let handle = tokio::runtime::Handle::current();
        let vector_coordinator = Arc::new(
            linkrs_sync::vector_sync::VectorSyncCoordinator::new_without_embedding(backend, handle),
        );

        // Create pipeline manager with vector coordinator and optional schema manager
        let mut pipeline_manager =
            Self::pipeline_with_engine(storage, stats_manager, optimizer_engine);

        if let Some(sm) = schema_manager {
            pipeline_manager = pipeline_manager
                .with_schema_manager(sm)
                .with_vector_coordinator(vector_coordinator);
        } else {
            pipeline_manager = pipeline_manager.with_vector_coordinator(vector_coordinator);
        }

        Ok(Self { pipeline_manager })
    }
}
