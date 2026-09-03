//! Query Plan Cache Module
//!
//! Provides Prepared Statement style query plan caching with support for parameterized queries.
//!
//! # Design objectives
//!
//! 1. Cache query plan parsing, validation and planning results
//! 2. Support for parameterized queries (Prepared Statement)
//! 3. Limit memory usage to prevent unlimited growth
//! 4. Thread-safe, supporting highly concurrent access
//!
//! # Scenarios of use
//!
//! - Repeated execution of the same query template
//! - Batch insert/update operations
//! - Applications use Prepared Statements

use moka::sync::Cache;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::executor::streaming::plan::PhysicalPlan;
use crate::planning::plan::execution_plan::PartitionSpec;
use graphdb_core::stats::StatsManager;

use super::config::{CachePriority, PlanCacheConfig};
use super::stats::PlanCacheStats;

pub mod entry;
pub mod key;
pub mod params;
#[cfg(test)]
mod tests;

pub use entry::CachedPlan;
pub use key::{ParamPosition, PlanCacheContext, PlanCacheKey, PlanCachePutContext};
pub use params::ParameterizedQueryHandler;

pub struct QueryPlanCache {
    /// Cache storage - using moka for high-performance concurrent access
    cache: Cache<PlanCacheKey, Arc<CachedPlan>>,
    /// Configuration
    config: PlanCacheConfig,
    /// Statistics
    stats: Arc<PlanCacheStats>,
    /// Stats manager for reporting cache metrics
    stats_manager: std::sync::RwLock<Option<Arc<StatsManager>>>,
}

impl std::fmt::Debug for QueryPlanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryPlanCache")
            .field("config", &self.config)
            .field("stats", &self.stats.snapshot())
            .finish()
    }
}

impl QueryPlanCache {
    /// Create a new query plan cache.
    pub fn new(config: PlanCacheConfig) -> Self {
        let max_weight = config.effective_max_weight();

        let cache = Cache::builder()
            .weigher(|_key, value: &Arc<CachedPlan>| -> u32 {
                let arc_overhead = std::mem::size_of::<Arc<CachedPlan>>();
                (value.estimate_memory() + arc_overhead) as u32
            })
            .max_capacity(max_weight)
            .time_to_live(Duration::from_secs(config.ttl_config.base_ttl_seconds))
            .build();

        let stats = Arc::new(PlanCacheStats::new(config.memory_budget));

        Self {
            cache,
            config,
            stats,
            stats_manager: std::sync::RwLock::new(None),
        }
    }

    pub fn with_stats_manager(self, stats_manager: Arc<StatsManager>) -> Self {
        if let Ok(mut guard) = self.stats_manager.write() {
            *guard = Some(stats_manager);
        }
        self
    }

    /// Set the stats manager after creation
    pub fn set_stats_manager(&self, stats_manager: Arc<StatsManager>) {
        if let Ok(mut guard) = self.stats_manager.write() {
            *guard = Some(stats_manager);
        }
    }

    /// Obtaining the cached plan
    ///
    /// # Parameters
    /// - `query`: The text of the query
    ///
    /// # Returns
    /// - `Some(Arc<CachedPlan>)`: Cached plan
    /// - `None`: No results were found, or there was a hash collision.
    pub fn get(&self, query: &str) -> Option<Arc<CachedPlan>> {
        self.get_with_space(query, None, None)
    }

    /// Look up a cached plan with space, schema, and parameter type context.
    ///
    /// M0: space_name and schema_version are incorporated into the cache key
    /// to prevent cross-space plan reuse and stale plans after DDL.
    ///
    /// M1.6: param_type_signature is a hash of parameter *types* so that
    /// the same query with different param types gets a different cache key.
    ///
    /// index_version forces replan after index DDL (use the same value
    /// that was passed to `put_with_context` to ensure key matching).
    pub fn get_with_space(
        &self,
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
    ) -> Option<Arc<CachedPlan>> {
        self.get_with_full_context(query, space_name, schema_version, None, None)
    }

    /// Look up with space, schema version, AND index version.
    ///
    /// the index_version dimension ensures that a plan compiled with a
    /// certain index state is not reused after index DDL.
    pub fn get_with_full_space(
        &self,
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
        index_version: Option<u64>,
    ) -> Option<Arc<CachedPlan>> {
        self.get_with_full_context(query, space_name, schema_version, None, index_version)
    }

    /// Full-context cache lookup with parameter type signature and index version.
    pub fn get_with_full_context(
        &self,
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
        param_type_signature: Option<u64>,
        index_version: Option<u64>,
    ) -> Option<Arc<CachedPlan>> {
        self.get_with_context(
            query,
            PlanCacheContext {
                space_name,
                schema_version,
                index_version,
                param_type_signature,
                ..Default::default()
            },
        )
    }

    pub fn get_with_context(
        &self,
        query: &str,
        context: PlanCacheContext,
    ) -> Option<Arc<CachedPlan>> {
        let key = PlanCacheKey::from_query_with_context(query, context);

        if let Some(plan) = self.cache.get(&key) {
            if plan.query_template != query {
                log::warn!(
                    "Query plan cache hash collision detected: hash={}, expected_query={}, cached_query={}",
                    key.hash,
                    query,
                    plan.query_template
                );
                self.stats.counters.record_miss();
                if let Ok(ref sm_guard) = self.stats_manager.read() {
                    if let Some(ref sm) = **sm_guard {
                        sm.record_cache_hit(0, false);
                    }
                }
                return None;
            }

            self.stats.counters.record_hit();
            if let Ok(ref sm_guard) = self.stats_manager.read() {
                if let Some(ref sm) = **sm_guard {
                    sm.record_cache_hit(0, true);
                }
            }
            return Some(plan);
        }

        self.stats.counters.record_miss();
        if let Ok(ref sm_guard) = self.stats_manager.read() {
            if let Some(ref sm) = **sm_guard {
                sm.record_cache_hit(0, false);
            }
        }
        None
    }

    /// Put the plan in the cache.
    ///
    /// M3: stores an [`Arc<PhysicalPlan>`] instead of [`ExecutionPlan`].
    ///
    /// # Parameters
    /// - `query`: Query text
    /// - `plan`: Arena-based physical plan
    /// - `param_positions`: Information about the positions of the parameters
    pub fn put(&self, query: &str, plan: Arc<PhysicalPlan>, param_positions: Vec<ParamPosition>) {
        self.put_with_context(query, plan, param_positions, PlanCachePutContext::default());
    }

    /// Put the plan in the cache with dependent tables.
    pub fn put_with_tables(
        &self,
        query: &str,
        plan: Arc<PhysicalPlan>,
        param_positions: Vec<ParamPosition>,
        dependent_tables: Vec<String>,
    ) {
        self.put_with_context(
            query,
            plan,
            param_positions,
            PlanCachePutContext {
                dependent_tables,
                ..PlanCachePutContext::default()
            },
        );
    }

    /// Put the plan with full context (space, schema version, index version, tables).
    ///
    /// M0: space_name and schema_version are incorporated into the cache key
    /// to prevent cross-space reuse and stale plans after DDL.
    ///
    /// M1.6: `param_type_signature` is derived from the parameter type
    /// declarations (not values) and is included in the cache key.
    ///
    /// index_version is incorporated into the cache key to force replan
    /// after index DDL (CREATE/DROP index) even when schema_version is unchanged.
    ///
    /// M3: stores [`Arc<PhysicalPlan>`] — the immutable arena plan.
    pub fn put_with_context(
        &self,
        query: &str,
        plan: Arc<PhysicalPlan>,
        param_positions: Vec<ParamPosition>,
        context: PlanCachePutContext,
    ) {
        let PlanCachePutContext {
            dependent_tables,
            space_name,
            schema_version,
            index_version,
            is_dml,
            is_transaction,
            optimizer_version,
            planning_config_hash,
        } = context;
        let query_bytes = query.len();
        let param_type_sig = Self::compute_param_type_signature(&param_positions);
        let key = PlanCacheKey::from_query_with_context(
            query,
            PlanCacheContext {
                space_name,
                schema_version,
                index_version,
                param_type_signature: param_type_sig,
                optimizer_version,
                planning_config_hash,
            },
        );

        let priority = if self.config.priority_config.enable_priority {
            self.calculate_priority(&plan)
        } else {
            CachePriority::Normal
        };

        let complexity_score = self.calculate_complexity_score(&plan);
        let estimated_compute_cost = self.estimate_compute_cost(&plan);
        let current_ttl = Duration::from_secs(self.config.ttl_config.base_ttl_seconds);

        let cached_plan = Arc::new(CachedPlan {
            query_template: query.to_string(),
            plan,
            param_positions,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
            avg_execution_time_ms: 0.0,
            execution_count: 0,
            priority,
            complexity_score,
            estimated_compute_cost,
            current_ttl,
            dependent_tables,
            is_dml,
            is_transaction,
        });

        let is_update = self.cache.contains_key(&key);
        self.cache.insert(key, cached_plan);

        if !is_update {
            self.stats.record_query_size(query_bytes);
        }

        let current_entries = self.cache.entry_count() as usize;
        let current_memory = self.estimate_current_memory();
        self.stats.memory.update(current_memory, current_entries);
    }

    /// Calculate priority based on query characteristics.
    ///
    /// M3: uses operator count from the arena [`PhysicalPlan`].
    fn calculate_priority(&self, plan: &PhysicalPlan) -> CachePriority {
        let complexity = self.calculate_complexity_score(plan);

        if complexity > 1000 {
            CachePriority::High
        } else if complexity > 100 {
            CachePriority::Normal
        } else {
            CachePriority::Low
        }
    }

    /// Calculate complexity score from the arena [`PhysicalPlan`].
    ///
    /// M3: uses operator count and fragment count as a proxy for complexity.
    fn calculate_complexity_score(&self, plan: &PhysicalPlan) -> u32 {
        let op_count = plan.operator_count() as u32;
        let frag_count = plan.fragment_count() as u32;
        (op_count * 20).max(frag_count * 10)
    }

    /// Estimate compute cost in milliseconds.
    fn estimate_compute_cost(&self, plan: &PhysicalPlan) -> u64 {
        let complexity = self.calculate_complexity_score(plan);
        (complexity as u64 * 10).max(100)
    }

    /// Estimate current memory usage
    fn estimate_current_memory(&self) -> usize {
        self.cache
            .iter()
            .map(|entry| entry.1.estimate_memory())
            .sum()
    }

    /// Look up a cached plan keyed by its physical partition layout.
    ///
    /// The key incorporates the layout fingerprint (ranges, source, layout
    /// version), so a plan cached under one layout is never reused for a
    /// different one.  This is the lookup used by callers that already know
    /// the current layout (e.g. re-planning against a previous execution's
    /// metadata); plain-text lookups deliberately miss partitioned entries.
    pub fn get_with_partition(&self, query: &str, spec: &PartitionSpec) -> Option<Arc<CachedPlan>> {
        self.get_with_partition_context(query, spec, PlanCacheContext::default())
    }

    /// Partition-keyed lookup that additionally scopes by the full
    /// compatibility context, matching [`put_with_partition`](Self::put_with_partition).
    pub fn get_with_partition_context(
        &self,
        query: &str,
        spec: &PartitionSpec,
        context: PlanCacheContext,
    ) -> Option<Arc<CachedPlan>> {
        let key = PlanCacheKey::from_query_with_partition_and_context(query, spec, context);

        if let Some(plan) = self.cache.get(&key) {
            if plan.query_template != query {
                log::warn!(
                    "Query plan cache hash collision detected: hash={}, expected_query={}, cached_query={}",
                    key.hash,
                    query,
                    plan.query_template
                );
                self.stats.counters.record_miss();
                return None;
            }
            self.stats.counters.record_hit();
            if let Ok(ref sm_guard) = self.stats_manager.read() {
                if let Some(ref sm) = **sm_guard {
                    sm.record_cache_hit(0, true);
                }
            }
            return Some(plan);
        }

        self.stats.counters.record_miss();
        if let Ok(ref sm_guard) = self.stats_manager.read() {
            if let Some(ref sm) = **sm_guard {
                sm.record_cache_hit(0, false);
            }
        }
        None
    }

    /// Store a plan under its physical partition layout key.
    ///
    /// Only partitioned plans should use this; the plain-text key is left
    /// free so a subsequent lookup that cannot provide the layout does not
    /// silently reuse a layout-dependent plan.
    pub fn put_with_partition(
        &self,
        query: &str,
        spec: &PartitionSpec,
        plan: Arc<PhysicalPlan>,
        param_positions: Vec<ParamPosition>,
        context: PlanCachePutContext,
    ) {
        let PlanCachePutContext {
            dependent_tables,
            space_name,
            schema_version,
            index_version,
            is_dml,
            is_transaction,
            optimizer_version,
            planning_config_hash,
        } = context;
        let param_type_sig = Self::compute_param_type_signature(&param_positions);
        let key = PlanCacheKey::from_query_with_partition_and_context(
            query,
            spec,
            PlanCacheContext {
                space_name,
                schema_version,
                index_version,
                param_type_signature: param_type_sig,
                optimizer_version,
                planning_config_hash,
            },
        );
        let priority = if self.config.priority_config.enable_priority {
            self.calculate_priority(&plan)
        } else {
            CachePriority::Normal
        };
        let complexity_score = self.calculate_complexity_score(&plan);
        let cached_plan = Arc::new(CachedPlan {
            query_template: query.to_string(),
            plan,
            param_positions,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
            avg_execution_time_ms: 0.0,
            execution_count: 0,
            priority,
            complexity_score,
            estimated_compute_cost: self.estimate_compute_cost_from_score(complexity_score),
            current_ttl: Duration::from_secs(self.config.ttl_config.base_ttl_seconds),
            dependent_tables,
            is_dml,
            is_transaction,
        });
        self.cache.insert(key, cached_plan);
        self.stats.memory.update(
            self.estimate_current_memory(),
            self.cache.entry_count() as usize,
        );
    }

    /// Estimate compute cost in milliseconds from an already-computed score.
    fn estimate_compute_cost_from_score(&self, complexity: u32) -> u64 {
        (complexity as u64 * 10).max(100)
    }

    /// Compute a parameter type signature from param positions.
    ///
    /// M1.6: produces a hash of the parameter *types* (not values) so that
    /// plans with different param type declarations get different cache keys.
    pub(crate) fn compute_param_type_signature(params: &[ParamPosition]) -> Option<u64> {
        if params.is_empty() {
            return None;
        }
        use std::hash::Hash;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for p in params {
            p.index.hash(&mut hasher);
            p.name.hash(&mut hasher);
            p.expected_type.hash(&mut hasher);
        }
        Some(hasher.finish())
    }

    /// Record the statistics on the execution of the plan.
    ///
    /// # Parameter
    /// - `query`: Query content
    /// - `execution_time_ms`: Execution time (in milliseconds)
    pub fn record_execution(&self, query: &str, execution_time_ms: f64) {
        let key = PlanCacheKey::from_query(query);

        if let Some(plan) = self.cache.get(&key) {
            let alpha = 0.1;
            let new_avg = plan.avg_execution_time_ms * (1.0 - alpha) + execution_time_ms * alpha;

            let updated_plan = Arc::new(CachedPlan {
                execution_count: plan.execution_count + 1,
                avg_execution_time_ms: new_avg,
                ..(*plan).clone()
            });

            self.cache.insert(key, updated_plan);
        }
    }

    /// Record execution with space context.
    ///
    /// index_version must match the value used during `put_with_context` to
    /// locate the cached plan; otherwise the keys will differ and the record
    /// will miss.
    ///
    /// `param_type_signature` must match the signature used during
    /// `put_with_context` (i.e. the hash of the parameter *types*) or the
    /// record will miss for parameterized queries.
    pub fn record_execution_with_space(
        &self,
        query: &str,
        execution_time_ms: f64,
        space_name: Option<String>,
        schema_version: Option<u64>,
        index_version: Option<u64>,
        param_type_signature: Option<u64>,
    ) {
        let key = PlanCacheKey::from_query_with_full_context(
            query,
            space_name,
            schema_version,
            param_type_signature,
            index_version,
        );

        if let Some(plan) = self.cache.get(&key) {
            let alpha = 0.1;
            let new_avg = plan.avg_execution_time_ms * (1.0 - alpha) + execution_time_ms * alpha;

            let updated_plan = Arc::new(CachedPlan {
                execution_count: plan.execution_count + 1,
                avg_execution_time_ms: new_avg,
                ..(*plan).clone()
            });

            self.cache.insert(key, updated_plan);
        }
    }

    /// Check whether the query has been cached.
    pub fn contains(&self, query: &str) -> bool {
        let key = PlanCacheKey::from_query(query);
        self.cache.contains_key(&key)
    }

    /// Invalidate the cache entry
    pub fn invalidate(&self, query: &str) -> bool {
        let key = PlanCacheKey::from_query(query);
        let removed = self.cache.remove(&key).is_some();

        if removed {
            self.stats.counters.record_eviction();
            self.update_stats();
        }

        removed
    }

    /// Invalidate all cache entries for a given space.
    ///
    /// M0: called after DDL/index/schema changes to force replanning.
    /// Iterates all entries and removes those matching the space name.
    pub fn invalidate_space(&self, space_name: &str) -> usize {
        // Collect keys to remove while iterating (can't mutate during iter).
        let keys_to_remove: Vec<PlanCacheKey> = self
            .cache
            .iter()
            .filter(|(k, _)| k.space_name.as_deref() == Some(space_name))
            .map(|(k, _)| PlanCacheKey {
                hash: k.hash,
                query_text: k.query_text.clone(),
                partition_fingerprint: k.partition_fingerprint,
                space_name: k.space_name.clone(),
                schema_version: k.schema_version,
                param_type_signature: k.param_type_signature,
                index_version: k.index_version,
                optimizer_version: k.optimizer_version,
                planning_config_hash: k.planning_config_hash,
            })
            .collect();
        let removed = keys_to_remove.len();
        for key in &keys_to_remove {
            self.cache.remove(key);
        }
        if removed > 0 {
            self.stats.counters.record_eviction();
            self.update_stats();
        }
        removed
    }

    /// Get cache entries for eviction (internal use)
    pub fn get_cache_entries(&self) -> Vec<(Arc<PlanCacheKey>, f64, usize)> {
        self.cache
            .iter()
            .map(|(k, v)| {
                let value_score = v.value_score();
                (k.clone(), value_score, v.query_template.len())
            })
            .collect()
    }

    /// Increment eviction count (internal use)
    pub fn increment_eviction_count(&self, count: u64) {
        for _ in 0..count {
            self.stats.counters.record_eviction();
        }
    }

    /// Clear all caches.
    pub fn clear(&self) {
        self.cache.invalidate_all();
        self.stats.reset();
    }

    /// Obtain statistical information
    pub fn stats(&self) -> Arc<PlanCacheStats> {
        self.stats.clone()
    }

    /// Record a hit without a cache lookup, for the Level 2 same-shape DML
    /// memo that short-circuits before `get_with_context`. Keeps the hit rate
    /// accounting consistent with the regular lookup path.
    pub fn record_memo_hit(&self) {
        self.stats.counters.record_hit();
        if let Ok(ref sm_guard) = self.stats_manager.read() {
            if let Some(ref sm) = **sm_guard {
                sm.record_cache_hit(0, true);
            }
        }
    }

    /// Get statistics snapshot
    pub fn stats_snapshot(&self) -> super::stats::PlanCacheStatsSnapshot {
        self.stats.snapshot()
    }

    /// Clean up expired entries.
    /// Note: moka handles TTL automatically, so this is a no-op
    pub fn cleanup_expired(&self) {
        // moka handles TTL automatically
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        self.cache.entry_count() as usize
    }

    /// Check whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.entry_count() == 0
    }

    /// Get the configuration
    pub fn config(&self) -> &PlanCacheConfig {
        &self.config
    }

    /// Update internal statistics
    fn update_stats(&self) {
        let current_entries = self.cache.entry_count() as usize;
        let current_memory = self.estimate_current_memory();
        self.stats.memory.update(current_memory, current_entries);
    }
}

impl Default for QueryPlanCache {
    fn default() -> Self {
        Self::new(PlanCacheConfig::default())
    }
}
