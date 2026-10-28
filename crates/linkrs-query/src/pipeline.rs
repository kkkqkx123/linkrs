mod compiler;
mod diagnostics;
mod execution;
mod frontend;
mod merge_conditional;
pub mod prepared;

use crate::cache::{ParameterizedQueryHandler, PlanCacheConfig, QueryPlanCache};
use crate::executor::streaming::plan::PhysicalPlan;
use crate::executor::streaming::pool::SharedScheduler;
use crate::executor::streaming::query_registry::QueryRegistry;
use crate::executor::streaming::SessionTransactionController;
use crate::optimizer::OptimizerEngine;
use crate::query_manager::QueryManager;
use crate::storage::QueryStorage;
use linkrs_core::metadata::index_manager::IndexMetadataManager;
use linkrs_core::metadata::SchemaManager;
#[cfg(feature = "fulltext")]
use linkrs_fulltext::manager::FulltextIndexManager;
use linkrs_metrics::StatsManager;
#[cfg(feature = "vector")]
use linkrs_sync::vector_sync::VectorSyncCoordinator;
use linkrs_sync::SyncManager;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Capacity of the DML shape caches below. Single-entry caching thrashed on
/// alternating shapes (A,B,A,B re-parsed and re-bound every time); a small
/// bounded map keeps the common bulk-load shapes resident while bounding
/// memory. Lookup is a linear scan, which is cheap at this size.
pub(crate) const DML_SHAPE_CACHE_CAP: usize = 16;

pub struct QueryPipelineManager<S: QueryStorage + 'static> {
    pub(crate) stats_manager: Arc<StatsManager>,
    pub(crate) optimizer_engine: Arc<OptimizerEngine>,
    pub(crate) plan_cache: Arc<QueryPlanCache>,
    pub(crate) param_handler: ParameterizedQueryHandler,
    pub(crate) schema_manager: Option<Arc<SchemaManager>>,
    pub(crate) index_manager: Option<Arc<dyn IndexMetadataManager>>,
    #[cfg(feature = "fulltext")]
    pub(crate) fulltext_manager: Option<Arc<FulltextIndexManager>>,
    #[cfg(feature = "vector")]
    pub(crate) vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pub(crate) storage: Option<Arc<RwLock<S>>>,
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) schema_generation: Arc<AtomicU64>,
    pub(crate) index_generation: Arc<AtomicU64>,
    pub(crate) query_registry: Option<Arc<QueryRegistry>>,
    /// Optional global QueryManager shared with the server so per-query runtimes
    /// can forward KILL/finish/progress events into one registry. None until the
    /// assembly installs it after the pipeline is built.
    pub(crate) query_manager: Option<Arc<QueryManager>>,
    /// Engine-level shared scheduler, created once and reused across queries.
    pub(crate) shared_scheduler: Option<Arc<SharedScheduler>>,
    pub(crate) session_controller: parking_lot::RwLock<Option<Arc<SessionTransactionController>>>,
    /// Shared macro catalog (user-defined macros), engine-wide.
    ///
    /// Created once with the pipeline manager and shared with every query
    /// through the binder (expansion), parser-adjacent planning and the
    /// execution runtime (DDL). Persistence/WAL backends can be attached by
    /// replacing this instance before serving queries.
    pub macro_manager: Arc<linkrs_core::metadata::MacroManager>,
    /// Shared type-alias catalog (user-defined types), engine-wide.
    ///
    /// Threaded the same way as [`Self::macro_manager`].
    pub type_alias_manager: Arc<linkrs_core::metadata::TypeAliasManager>,
    /// Shared sequence catalog (named sequences for `curr_val`/`next_val`),
    /// engine-wide.
    ///
    /// Threaded the same way as [`Self::macro_manager`].
    pub sequence_manager: Arc<linkrs_core::metadata::SequenceManager>,
    /// Serializes statistics collection to prevent concurrent re-collection.
    pub(crate) statistics_collect_lock: Arc<parking_lot::Mutex<()>>,
    /// Microseconds spent in the last statistics collection pass.
    pub(crate) last_statistics_collection_us: AtomicU64,
    /// Times a non-forced collection was skipped due to lock contention.
    pub(crate) statistics_collection_skips: AtomicU64,
    /// Sample cap for per-tag/per-edge-type degree estimation during collection.
    pub(crate) statistics_sample_limit: usize,
    /// Minimum data-epoch advance that triggers an automatic refresh.
    ///
    /// The epoch bumps on every write, so without a threshold any commit
    /// invalidates statistics and the next query pays a full re-collection.
    /// Smaller advances reuse the last collection; explicit ANALYZE still
    /// forces a refresh.
    pub(crate) statistics_min_epoch_delta: u64,
    /// Display label of the global cost profile (e.g. `Ssd`); empty until
    /// the assembly provides one. Only used for EXPLAIN observability.
    pub(crate) default_cost_profile_label: String,
    /// Display labels of per-space cost profile overrides (space -> label).
    pub(crate) space_cost_profile_labels: HashMap<String, String>,
    pub(crate) dml_shape_cache_enabled: bool,
    /// Fast path for repeated same-shape DML: skips bind and serves the last
    /// compiled physical plans directly. Bounded multi-entry map keyed by
    /// [`DmlPlanMemoKey`]; see that struct for the key dimensions.
    pub(crate) dml_plan_memo: parking_lot::Mutex<Vec<DmlPlanMemoEntry>>,
    pub last_dml_plan_hits: std::sync::atomic::AtomicU64,
    pub(crate) dml_memo_compat_failures: std::sync::atomic::AtomicU64,
    /// Parsed ASTs of normalized DML templates (`INSERT ... VALUES
    /// (@__dml_0, ...)`), avoiding a re-parse per execution. Bounded
    /// multi-entry map keyed by exact normalized text.
    pub(crate) dml_template_ast:
        parking_lot::Mutex<Vec<(String, Arc<crate::parser::ast::stmt::Ast>)>>,
    pub(crate) dml_template_ast_parse_count: std::sync::atomic::AtomicU64,
    pub(crate) dml_template_ast_hit_count: std::sync::atomic::AtomicU64,
    pub(crate) dml_bind_skipped_count: std::sync::atomic::AtomicU64,
    /// Retained password-history depth threaded into every query context so
    /// user-management plans rotate history uniformly across channels.
    pub(crate) password_history_depth: usize,
}

/// Lookup key for one [`DmlPlanMemoEntry`].
///
/// Mirrors the [`crate::cache::PlanCacheKey`] dimensions that are computable
/// *before* binding (the whole point of the memo is to skip bind): query
/// shape, space, schema/index generations, parameter type signature, and the
/// optimizer/planning configuration. The post-planning partition fingerprint
/// used by the plan cache cannot be part of this key; instead memo hits are
/// validated with [`crate::executor::streaming::plan::PhysicalPlanValidator::check_compatibility`]
/// before being served.
/// Memoized DML plan key: a fingerprint-less plan key over shape-normalized text.
///
/// Constructed only via [`DmlPlanMemoKey::from_normalized`] so the prepare-time
/// bind-skip probe and the compile-time serve/store paths stay identical.
/// Dimensions are the unified invalidation set (space, schema generation,
/// index generation, optimizer/cost version, planning config hash, parameter
/// type signature); the partition fingerprint is absent by construction and
/// covered post-hoc by the single compatibility exit
/// (`QueryPlanCache::serve_compatible`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DmlPlanMemoKey {
    pub inner: crate::cache::PlanCacheKey,
}

impl DmlPlanMemoKey {
    pub fn from_context(normalized_text: String, context: crate::cache::PlanCacheContext) -> Self {
        Self {
            inner: crate::cache::PlanCacheKey::fingerprint_less_from_context(
                &normalized_text,
                context,
            ),
        }
    }
}

pub(crate) struct DmlPlanMemoEntry {
    pub key: DmlPlanMemoKey,
    pub plan: Arc<PhysicalPlan>,
}

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    /// Fallback sampling window used when the server assembly did not pass
    /// an explicit `statistics_sample_limit` (mirrors the config default).
    pub const DEFAULT_STATISTICS_SAMPLE_LIMIT: usize = 10_000;
    /// Fallback epoch delta used when the server assembly did not pass an
    /// explicit `statistics_min_epoch_delta` (mirrors the config default).
    pub const DEFAULT_STATISTICS_MIN_EPOCH_DELTA: u64 = 100;

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
            #[cfg(feature = "fulltext")]
            fulltext_manager: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(1)),
            index_generation: Arc::new(AtomicU64::new(1)),
            query_registry: None,
            shared_scheduler: None,
            query_manager: None,
            macro_manager: Arc::new(linkrs_core::metadata::MacroManager::new()),
            type_alias_manager: Arc::new(linkrs_core::metadata::TypeAliasManager::new()),
            sequence_manager: Arc::new(linkrs_core::metadata::SequenceManager::new()),
            session_controller: parking_lot::RwLock::new(None),
            statistics_collect_lock: Arc::new(parking_lot::Mutex::new(())),
            last_statistics_collection_us: AtomicU64::new(0),
            statistics_collection_skips: AtomicU64::new(0),
            statistics_sample_limit: Self::DEFAULT_STATISTICS_SAMPLE_LIMIT,
            statistics_min_epoch_delta: Self::DEFAULT_STATISTICS_MIN_EPOCH_DELTA,
            default_cost_profile_label: String::new(),
            space_cost_profile_labels: HashMap::new(),
            dml_shape_cache_enabled: true,
            dml_plan_memo: parking_lot::Mutex::new(Vec::new()),
            last_dml_plan_hits: std::sync::atomic::AtomicU64::new(0),
            dml_memo_compat_failures: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast: parking_lot::Mutex::new(Vec::new()),
            dml_template_ast_parse_count: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast_hit_count: std::sync::atomic::AtomicU64::new(0),
            dml_bind_skipped_count: std::sync::atomic::AtomicU64::new(0),
            password_history_depth: 0,
        }
    }

    pub fn with_statistics_sample_limit(mut self, sample_limit: usize) -> Self {
        self.statistics_sample_limit = sample_limit.max(1);
        self
    }

    pub fn with_statistics_min_epoch_delta(mut self, delta: u64) -> Self {
        self.statistics_min_epoch_delta = delta;
        self
    }

    /// Set the retained password-history depth for user-management
    /// operations (assembly-time only).
    pub fn with_password_history_depth(mut self, depth: usize) -> Self {
        self.password_history_depth = depth;
        self
    }

    /// Replace the optimizer engine (assembly-time only).
    ///
    /// Re-runs the post-construction injection so the replacement behaves
    /// like an engine passed to [`Self::with_optimizer`]. Callers must swap
    /// before the first query; execution-time replacement is unsupported.
    pub fn set_optimizer_engine(&mut self, optimizer_engine: Arc<OptimizerEngine>) {
        optimizer_engine.set_cte_cache_stats_manager(self.stats_manager.clone());
        self.optimizer_engine = optimizer_engine;
    }

    /// Effective sampling window currently in force for statistics
    /// collection (server config value or the fallback default).
    pub fn statistics_sample_limit(&self) -> usize {
        self.statistics_sample_limit
    }

    /// Effective epoch delta currently in force for automatic refresh.
    pub fn statistics_min_epoch_delta(&self) -> u64 {
        self.statistics_min_epoch_delta
    }

    /// One-line summary of the effective statistics settings, surfaced in
    /// EXPLAIN diagnostics so operators can verify which sampling window
    /// and refresh threshold produced the statistics behind a plan.
    pub fn statistics_settings_summary(&self) -> String {
        format!(
            "sample_limit={}, min_epoch_delta={}",
            self.statistics_sample_limit, self.statistics_min_epoch_delta
        )
    }

    /// Install per-space cost model overrides on the engine, with display
    /// labels for EXPLAIN observability (assembly-time only).
    pub fn set_space_cost_configs(
        &mut self,
        configs: HashMap<String, crate::optimizer::CostModelConfig>,
        labels: HashMap<String, String>,
    ) {
        self.optimizer_engine.set_space_cost_configs(configs);
        self.space_cost_profile_labels = labels;
    }

    /// Display label of the global cost profile (assembly-time only).
    pub fn set_default_cost_profile_label(&mut self, label: String) {
        self.default_cost_profile_label = label;
    }

    /// Effective cost profile label for a space, if the assembly provided
    /// one: the space override when present, else the global label.
    /// Empty global label means unassembled: no label at all.
    pub fn cost_profile_label_for(&self, space: Option<&str>) -> Option<String> {
        if self.default_cost_profile_label.is_empty() {
            return None;
        }
        let label = space
            .and_then(|name| self.space_cost_profile_labels.get(name))
            .unwrap_or(&self.default_cost_profile_label);
        Some(label.clone())
    }

    /// Collect (or serve cached) statistics for a space into the optimizer's
    /// `StatisticsManager`.
    ///
    /// `force` bypasses the version gate so an explicit ANALYZE always
    /// refreshes the statistics. Collection is serialized internally; a
    /// non-forced pass that loses the `try_lock` race skips collection and
    /// returns a cached summary instead of blocking the query thread.
    pub fn collect_statistics(
        &self,
        space: &str,
        force: bool,
    ) -> Result<crate::optimizer::stats::CollectedSummary, String> {
        let start = std::time::Instant::now();
        let _guard = if force {
            self.statistics_collect_lock.lock()
        } else {
            match self.statistics_collect_lock.try_lock() {
                Some(guard) => guard,
                None => {
                    self.statistics_collection_skips
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    log::debug!(
                        "Statistics collection for space '{}' skipped: collector busy",
                        space
                    );
                    return Ok(crate::optimizer::stats::CollectedSummary {
                        cached: true,
                        ..Default::default()
                    });
                }
            }
        };
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
        self.last_statistics_collection_us.store(
            start.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
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

    /// Ensure statistics for `space` are fresh before planning.
    ///
    /// Missing stamps trigger one best-effort collection; small data-epoch
    /// advances below `statistics_min_epoch_delta` reuse the last collection
    /// so write-heavy workloads do not re-sample on every commit. Failures
    /// degrade to no-statistics fallbacks and never block the query. Explicit
    /// ANALYZE remains the manual forced-refresh entry via
    /// `collect_statistics(space, true)`.
    pub(crate) fn ensure_statistics(&self, space: &str) {
        let stats_manager = self.optimizer_engine.stats_manager();
        let schema_version = self
            .schema_generation
            .load(std::sync::atomic::Ordering::Relaxed);
        let data_epoch = match self.storage.as_ref() {
            Some(storage) => storage.read().stats_epoch(),
            None => return,
        };
        if schema_version != 0 && data_epoch != 0 {
            match stats_manager.space_stamp(space) {
                None => {}
                Some((stamped_schema, stamped_epoch))
                    if stamped_schema == schema_version && stamped_epoch == data_epoch =>
                {
                    return;
                }
                Some((stamped_schema, stamped_epoch))
                    if stamped_schema == schema_version
                        && data_epoch > stamped_epoch
                        && data_epoch - stamped_epoch < self.statistics_min_epoch_delta =>
                {
                    return;
                }
                _ => {}
            }
        }
        if let Err(error) = self.collect_statistics(space, false) {
            log::warn!(
                "Statistics refresh for space '{}' failed, degrading to no-stats: {}",
                space,
                error
            );
        }
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

    /// Set the global QueryManager (used when the pipeline is built before the
    /// manager is created, e.g. the server startup path).
    pub fn set_query_manager(&mut self, manager: Option<Arc<QueryManager>>) {
        self.query_manager = manager;
    }

    /// Access the global QueryManager instance, if installed.
    pub fn query_manager(&self) -> Option<Arc<QueryManager>> {
        self.query_manager.clone()
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
            #[cfg(feature = "fulltext")]
            fulltext_manager: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            storage: Some(storage),
            sync_manager: None,
            schema_generation: Arc::new(AtomicU64::new(1)),
            index_generation: Arc::new(AtomicU64::new(1)),
            query_registry: None,
            shared_scheduler: None,
            query_manager: None,
            macro_manager: Arc::new(linkrs_core::metadata::MacroManager::new()),
            type_alias_manager: Arc::new(linkrs_core::metadata::TypeAliasManager::new()),
            sequence_manager: Arc::new(linkrs_core::metadata::SequenceManager::new()),
            session_controller: parking_lot::RwLock::new(None),
            statistics_collect_lock: Arc::new(parking_lot::Mutex::new(())),
            last_statistics_collection_us: AtomicU64::new(0),
            statistics_collection_skips: AtomicU64::new(0),
            statistics_sample_limit: Self::DEFAULT_STATISTICS_SAMPLE_LIMIT,
            statistics_min_epoch_delta: Self::DEFAULT_STATISTICS_MIN_EPOCH_DELTA,
            default_cost_profile_label: String::new(),
            space_cost_profile_labels: HashMap::new(),
            dml_shape_cache_enabled: true,
            dml_plan_memo: parking_lot::Mutex::new(Vec::new()),
            last_dml_plan_hits: std::sync::atomic::AtomicU64::new(0),
            dml_memo_compat_failures: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast: parking_lot::Mutex::new(Vec::new()),
            dml_template_ast_parse_count: std::sync::atomic::AtomicU64::new(0),
            dml_template_ast_hit_count: std::sync::atomic::AtomicU64::new(0),
            dml_bind_skipped_count: std::sync::atomic::AtomicU64::new(0),
            password_history_depth: 0,
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

    #[cfg(feature = "fulltext")]
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
        // The DML shape caches feed plan construction, so a full clear must
        // cover them too; otherwise `clear_plan_cache` would leave the bind
        // fast path serving plans the plan cache itself just dropped.
        self.dml_plan_memo.lock().clear();
        self.dml_template_ast.lock().clear();
        log::info!("Query plan cache cleared");
    }

    pub fn with_sync_manager(mut self, sync_manager: Arc<SyncManager>) -> Self {
        // The fulltext SQL path (planning + management) reads the manager;
        // surface it from the sync manager so `SEARCH INDEX` resolves.
        #[cfg(feature = "fulltext")]
        if let Some(coordinator) = sync_manager.fulltext_coordinator() {
            self.fulltext_manager = Some(coordinator.fulltext_manager().clone());
        }
        // Prefer the sync-manager-hosted vector coordinator so the query
        // plan and the HTTP/gRPC vector APIs share one source of truth.
        // A `with_vector_backend` coordinator installed earlier stays only
        // as a fallback when the sync manager has none.
        #[cfg(feature = "vector")]
        if let Some(coordinator) = sync_manager.vector_coordinator() {
            self.vector_coordinator = Some(coordinator.clone());
        }
        self.sync_manager = Some(sync_manager);
        self
    }

    /// Vector coordinator currently backing query planning, if any.
    #[cfg(feature = "vector")]
    pub fn vector_coordinator(&self) -> Option<Arc<VectorSyncCoordinator>> {
        self.vector_coordinator.clone()
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

    pub fn dml_template_ast_hit_count(&self) -> u64 {
        self.dml_template_ast_hit_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dml_bind_skipped_count(&self) -> u64 {
        self.dml_bind_skipped_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dml_memo_compat_failure_count(&self) -> u64 {
        self.dml_memo_compat_failures
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Microseconds spent in the last statistics collection pass.
    pub fn last_statistics_collection_us(&self) -> u64 {
        self.last_statistics_collection_us
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Times a non-forced collection was skipped due to lock contention.
    pub fn statistics_collection_skips(&self) -> u64 {
        self.statistics_collection_skips
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The underlying storage binding, if any.
    pub fn storage(&self) -> Option<Arc<RwLock<S>>> {
        self.storage.clone()
    }

    /// Planning configuration snapshot used for DML memo keys. Must stay
    /// identical to the config built in `compile_or_get_cached`; both sides
    /// of the memo (prepare-time lookup, compile-time store) derive their
    /// `optimizer_version` / `planning_config_hash` from here so the keys
    /// stay comparable.
    pub(crate) fn dml_planning_config(
        &self,
    ) -> crate::executor::streaming::plan::context::PlanningConfig {
        crate::executor::streaming::plan::context::PlanningConfig {
            max_partitions: self
                .optimizer_engine
                .partitioning_config()
                .max_workers
                .max(1),
            config_hash: Self::partitioning_config_hash(
                self.optimizer_engine.partitioning_config(),
            ),
            // Any cost configuration change (global or per-space) retires
            // cached plans priced under the old configuration.
            optimizer_version: self.optimizer_engine.cost_epoch(),
            ..Default::default()
        }
    }

    /// Extract parameter positions for a (possibly shape-normalized) query
    /// text and fill each position's expected type from the request
    /// parameters. Shared by the plan cache put path and the memo signature
    /// so both derive from one construction.
    pub(crate) fn typed_param_positions(
        &self,
        normalized_text: &str,
        parameters: &std::collections::HashMap<String, linkrs_core::Value>,
    ) -> Vec<crate::cache::ParamPosition> {
        let mut positions = self.param_handler.extract_params(normalized_text);
        for position in &mut positions {
            let name = position
                .name
                .clone()
                .unwrap_or_else(|| position.index.to_string());
            position.expected_type = parameters.get(&name).map(|value| value.data_type());
        }
        positions
    }

    /// Parameter type signature for a (possibly shape-normalized) query text,
    /// using the same extract + `compute_param_type_signature` path as the
    /// plan cache so memo and plan keys share one construction.
    pub(crate) fn memo_param_signature(
        &self,
        normalized_text: &str,
        parameters: &std::collections::HashMap<String, linkrs_core::Value>,
    ) -> Option<u64> {
        let positions = self.typed_param_positions(normalized_text, parameters);
        crate::cache::plan_cache::QueryPlanCache::compute_param_type_signature(&positions)
    }

    /// Single construction of the plan-cache identity context shared by the
    /// memo key, the plan-cache lookup and the plan-cache put paths, so every
    /// cache key derives its dimensions from one place.
    pub(crate) fn plan_cache_context(
        &self,
        space_name: Option<String>,
        param_type_signature: Option<u64>,
    ) -> crate::cache::PlanCacheContext {
        let planning_config = self.dml_planning_config();
        crate::cache::PlanCacheContext {
            space_name,
            schema_version: Some(
                self.schema_generation
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            index_version: Some(
                self.index_generation
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            param_type_signature,
            optimizer_version: planning_config.optimizer_version,
            planning_config_hash: planning_config.config_hash,
        }
    }

    /// Single memo-key constructor shared by the prepare-time probe and the
    /// compile-time serve/store paths.
    pub(crate) fn dml_memo_key_for(
        &self,
        normalized_text: String,
        space_name: Option<String>,
        parameters: &std::collections::HashMap<String, linkrs_core::Value>,
    ) -> DmlPlanMemoKey {
        let param_sig = self.memo_param_signature(&normalized_text, parameters);
        DmlPlanMemoKey::from_context(
            normalized_text,
            self.plan_cache_context(space_name, param_sig),
        )
    }

    /// Look up a parsed normalized-DML template AST. Hits are moved to the
    /// front (cheap LRU approximation for the linear-scan map).
    pub(crate) fn lookup_dml_template(
        &self,
        normalized_text: &str,
    ) -> Option<Arc<crate::parser::ast::stmt::Ast>> {
        let mut guard = self.dml_template_ast.lock();
        let pos = guard.iter().position(|(text, _)| text == normalized_text)?;
        if pos > 0 {
            let entry = guard.remove(pos);
            guard.insert(0, entry);
        }
        self.dml_template_ast_hit_count
            .fetch_add(1, Ordering::Relaxed);
        Some(guard[0].1.clone())
    }

    /// Store a parsed normalized-DML template AST, evicting the least
    /// recently used entry beyond [`DML_SHAPE_CACHE_CAP`].
    pub(crate) fn store_dml_template(
        &self,
        normalized_text: String,
        ast: Arc<crate::parser::ast::stmt::Ast>,
    ) {
        let mut guard = self.dml_template_ast.lock();
        guard.retain(|(text, _)| text != &normalized_text);
        guard.insert(0, (normalized_text, ast));
        guard.truncate(DML_SHAPE_CACHE_CAP);
    }

    /// Look up a memoized DML physical plan by full key. Serves only as a
    /// candidate: callers must still validate the plan with
    /// `PhysicalPlanValidator::check_compatibility` before serving it, since
    /// the memo key cannot capture post-planning layout changes.
    pub(crate) fn find_dml_plan(&self, key: &DmlPlanMemoKey) -> Option<Arc<PhysicalPlan>> {
        let mut guard = self.dml_plan_memo.lock();
        let pos = guard.iter().position(|entry| entry.key == *key)?;
        if pos > 0 {
            let entry = guard.remove(pos);
            guard.insert(0, entry);
        }
        Some(guard[0].plan.clone())
    }

    /// Store a memoized DML physical plan, replacing any same-key entry and
    /// evicting the least recently used entry beyond [`DML_SHAPE_CACHE_CAP`].
    /// Stale keys (e.g. from before a schema-generation bump) never match
    /// new lookups and age out through this cap.
    pub(crate) fn store_dml_plan(&self, key: DmlPlanMemoKey, plan: Arc<PhysicalPlan>) {
        let mut guard = self.dml_plan_memo.lock();
        guard.retain(|entry| entry.key != key);
        guard.insert(0, DmlPlanMemoEntry { key, plan });
        guard.truncate(DML_SHAPE_CACHE_CAP);
    }
}
