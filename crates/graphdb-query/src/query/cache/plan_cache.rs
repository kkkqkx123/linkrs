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

use crate::core::stats::StatsManager;
use crate::query::executor::streaming::plan::PhysicalPlan;

use super::config::{CachePriority, PlanCacheConfig};
use super::stats::PlanCacheStats;

/// Parameter location information
#[derive(Debug, Clone)]
pub struct ParamPosition {
    /// Parameter Index
    pub index: usize,
    /// Parameter name (named parameter)
    pub name: Option<String>,
    /// Position of the parameter in the query
    pub position: usize,
    /// Desired data types
    pub expected_type: Option<crate::core::types::DataType>,
}

/// Context stored with a cached plan.
#[derive(Debug, Clone, Default)]
pub struct PlanCachePutContext {
    pub dependent_tables: Vec<String>,
    pub space_name: Option<String>,
    pub schema_version: Option<u64>,
    pub index_version: Option<u64>,
    pub is_dml: bool,
    pub is_transaction: bool,
    pub optimizer_version: u64,
    pub planning_config_hash: u64,
    pub capability_set: u64,
}

/// Complete request/catalog/runtime context used for every plan-cache operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCacheContext {
    pub space_name: Option<String>,
    pub schema_version: Option<u64>,
    pub index_version: Option<u64>,
    pub param_type_signature: Option<u64>,
    pub optimizer_version: u64,
    pub planning_config_hash: u64,
    pub capability_set: u64,
}

use crate::query::planning::plan::execution_plan::{PartitionSource, PartitionSpec};

/// Query Plan Cache Key
///
/// Supports fast lookups using the hash of the query text as the key.
/// Also store query text for conflict detection.
///
/// When a plan carries a non-stale `PartitionSpec` the key includes a
/// partition fingerprint so that a change in the underlying data layout
/// (e.g. re-indexing) automatically yields a cache miss, forcing the
/// planner to produce a fresh physical plan.  Callers that only have
/// the query text (the common lookup path before planning) query with
/// `fingerprint == None`, which will *not* match entries that were stored
/// with a fingerprint — effectively isolating partitioned plans from the
/// cache until the caller can provide the current layout version.
///
/// **M0**: Cache key now includes optional `space_name` and `schema_version`
/// to prevent cross-space plan reuse and stale plans after schema changes.
///
/// **M1.6**: Cache key includes optional `param_type_signature` so that the
/// same query text with different parameter type signatures produces a
/// different key, but different parameter *values* do not (allowing cached
/// plan reuse across executions with different values of the same types).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanCacheKey {
    /// Query the hash value of the text
    pub hash: u64,
    /// Query text (for conflict detection, not just debugging)
    query_text: String,
    /// Partition layout fingerprint.  `Some` when the cached plan holds a
    /// `PartitionSpec`; absent for single-tree plans.
    partition_fingerprint: Option<u64>,
    /// Space/catalog identity — prevents cross-space plan reuse (M0).
    space_name: Option<String>,
    /// Schema version at planning time — forces replan after DDL (M0).
    schema_version: Option<u64>,
    /// Parameter type signature — prevents reuse when param types differ (M1.6).
    /// Does NOT include parameter values, only their declared types.
    param_type_signature: Option<u64>,
    /// Index version at planning time — forces replan after index DDL (P2).
    index_version: Option<u64>,
    optimizer_version: u64,
    planning_config_hash: u64,
    capability_set: u64,
}

impl PlanCacheKey {
    /// Creating Cache Keys from Query Text
    pub fn from_query(query: &str) -> Self {
        let hash = Self::hash_query(query);
        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: None,
            space_name: None,
            schema_version: None,
            param_type_signature: None,
            index_version: None,
            optimizer_version: 0,
            planning_config_hash: 0,
            capability_set: 0,
        }
    }

    /// Create a key scoped to a specific space and schema version.
    ///
    /// M1.6: `param_type_signature` is a hash of the parameter types (not
    /// values) so the same query with different param values but same types
    /// reuses the cached plan.
    pub fn from_query_with_space(
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
    ) -> Self {
        Self::from_query_with_full_context(query, space_name, schema_version, None, None)
    }

    /// Create a key with all available context dimensions.
    pub fn from_query_with_full_context(
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
        param_type_signature: Option<u64>,
        index_version: Option<u64>,
    ) -> Self {
        Self::from_query_with_context(
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

    pub fn from_query_with_context(query: &str, context: PlanCacheContext) -> Self {
        use std::hash::Hash;

        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        if let Some(ref name) = context.space_name {
            name.hash(&mut hasher);
        }
        context.schema_version.hash(&mut hasher);
        context.param_type_signature.hash(&mut hasher);
        context.index_version.hash(&mut hasher);
        context.optimizer_version.hash(&mut hasher);
        context.planning_config_hash.hash(&mut hasher);
        context.capability_set.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: None,
            space_name: context.space_name,
            schema_version: context.schema_version,
            param_type_signature: context.param_type_signature,
            index_version: context.index_version,
            optimizer_version: context.optimizer_version,
            planning_config_hash: context.planning_config_hash,
            capability_set: context.capability_set,
        }
    }

    /// Create a key that additionally captures a physical partition layout,
    /// preventing stale cached plans from being reused after layout changes.
    pub fn from_query_with_partition(query: &str, spec: &PartitionSpec) -> Self {
        use std::hash::Hash;

        let fp = Self::compute_fingerprint(spec);

        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        fp.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: Some(fp),
            space_name: None,
            schema_version: None,
            param_type_signature: None,
            index_version: None,
            optimizer_version: 0,
            planning_config_hash: 0,
            capability_set: 0,
        }
    }

    fn hash_query(query: &str) -> u64 {
        use std::hash::Hash;
        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        hasher.finish()
    }

    /// Canonicalize insignificant whitespace for cache identity while retaining
    /// the original query text for collision diagnostics.
    fn normalize_query(query: &str) -> String {
        query.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn hasher() -> std::collections::hash_map::DefaultHasher {
        std::collections::hash_map::DefaultHasher::new()
    }

    /// Produce a numeric fingerprint from the partition ranges, source, and
    /// layout version so that any change in the range layout, the data domain
    /// it maps over, or the storage layout version yields a different key.
    fn compute_fingerprint(spec: &PartitionSpec) -> u64 {
        use std::hash::Hash;
        let mut hasher = Self::hasher();
        for range in spec.ranges() {
            range.start.hash(&mut hasher);
            range.end.hash(&mut hasher);
        }
        spec.source().to_string().hash(&mut hasher);
        spec.layout_version().hash(&mut hasher);
        hasher.finish()
    }

    /// Verify that the query text matches (for conflict detection)
    pub fn verify_query(&self, query: &str) -> bool {
        self.query_text == query
    }

    /// Get query text (for debugging or logging)
    pub fn query_text(&self) -> &str {
        &self.query_text
    }

    /// Whether this key holds a partition fingerprint.
    pub fn has_partition_fingerprint(&self) -> bool {
        self.partition_fingerprint.is_some()
    }

    /// Return the space name, if any.
    pub fn space_name(&self) -> Option<&str> {
        self.space_name.as_deref()
    }

    /// Return the schema version, if any.
    pub fn schema_version(&self) -> Option<u64> {
        self.schema_version
    }
}

/// Cached query plan entries
///
/// M3: stores [`Arc<PhysicalPlan>`] instead of [`ExecutionPlan`] so that the
/// cached plan is an immutable, verifiable arena plan that can be shared
/// across concurrent executions without re-building.
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// Query template (parameterized form)
    pub query_template: String,
    /// Immutable arena-based physical plan (M3).
    pub plan: Arc<PhysicalPlan>,
    /// Parameter location information (for parameter binding)
    pub param_positions: Vec<ParamPosition>,
    /// Creation time
    pub created_at: Instant,
    /// Last access time
    pub last_accessed: Instant,
    /// Number of visits
    pub access_count: u64,
    /// Average execution time (milliseconds)
    pub avg_execution_time_ms: f64,
    /// Number of executions
    pub execution_count: u64,
    /// Cache priority
    pub priority: CachePriority,
    /// Plan complexity score (for eviction decisions)
    pub complexity_score: u32,
    /// Estimated compute cost (milliseconds)
    pub estimated_compute_cost: u64,
    /// Current TTL
    pub current_ttl: Duration,
    /// Dependent tables (for invalidation)
    pub dependent_tables: Vec<String>,
    /// Whether the plan performs DML (requires a write transaction scope).
    pub is_dml: bool,
    /// Whether the plan is a transaction control statement (BEGIN/COMMIT/ROLLBACK).
    pub is_transaction: bool,
}

impl CachedPlan {
    /// Estimate memory usage (bytes)
    pub fn estimate_memory(&self) -> usize {
        let mut total = 0;

        total += std::mem::size_of::<String>();
        total += self.query_template.capacity();

        total += std::mem::size_of::<Vec<ParamPosition>>();
        for pos in &self.param_positions {
            total += std::mem::size_of::<ParamPosition>();
            if let Some(ref name) = pos.name {
                total += std::mem::size_of::<String>();
                total += name.capacity();
            }
        }

        total += self.estimate_plan_memory(&self.plan);

        total += std::mem::size_of::<Instant>() * 2;
        total += std::mem::size_of::<u64>() * 3;
        total += std::mem::size_of::<f64>() * 2;
        total += std::mem::size_of::<CachePriority>();
        total += std::mem::size_of::<u32>();
        total += std::mem::size_of::<Duration>();

        total += std::mem::size_of::<Vec<String>>();
        for table in &self.dependent_tables {
            total += std::mem::size_of::<String>();
            total += table.capacity();
        }

        total
    }

    /// Estimate memory usage for the physical plan.
    ///
    /// M3: uses [`PhysicalPlan::operator_count`] and spec sizes for estimation.
    fn estimate_plan_memory(&self, plan: &PhysicalPlan) -> usize {
        let base_size = std::mem::size_of::<PhysicalPlan>();
        let op_count = plan.operator_count();
        let fragment_count = plan.fragment_count();
        let per_op_estimate = 256; // rough estimate per operator spec

        base_size + (op_count * per_op_estimate) + (fragment_count * 64)
    }

    /// Calculate cache value score (for eviction decisions)
    pub fn value_score(&self) -> f64 {
        let frequency_score = self.access_count as f64 * 0.4;
        let cost_score = (self.estimated_compute_cost as f64 / 1000.0) * 0.3;
        let priority_score = (self.priority as i32 as f64) * 0.2;
        let size_penalty = (self.query_template.len() as f64 / 1024.0) * 0.1;

        frequency_score + cost_score + priority_score - size_penalty
    }
}

/// Query plan cache
///
/// Provide a query plan cache in the style of a Prepared Statement
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
    /// P2: index_version forces replan after index DDL (use the same value
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
    /// P2: the index_version dimension ensures that a plan compiled with a
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
    /// M3: stores an [`Arc<PhysicalPlan>`] instead of the legacy [`ExecutionPlan`].
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
    /// P2: index_version is incorporated into the cache key to force replan
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
            capability_set,
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
                capability_set,
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
        let key = PlanCacheKey::from_query_with_partition(query, spec);

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
            return Some(plan);
        }

        self.stats.counters.record_miss();
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
    ) {
        let key = PlanCacheKey::from_query_with_partition(query, spec);
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
            dependent_tables: Vec::new(),
            is_dml: false,
            is_transaction: false,
        });
        self.cache.insert(key, cached_plan);
        self.stats.memory.update(self.estimate_current_memory(), self.cache.entry_count() as usize);
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
    /// P2: index_version must match the value used during `put_with_context` to
    /// locate the cached plan; otherwise the keys will differ and the record
    /// will miss.
    pub fn record_execution_with_space(
        &self,
        query: &str,
        execution_time_ms: f64,
        space_name: Option<String>,
        schema_version: Option<u64>,
        index_version: Option<u64>,
    ) {
        let key = PlanCacheKey::from_query_with_full_context(
            query,
            space_name,
            schema_version,
            None,
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
                capability_set: k.capability_set,
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

/// Parameterized query processor
///
/// Handling the parsing and binding of parameterized queries
pub struct ParameterizedQueryHandler {
    /// Parameter placeholder pattern
    placeholder_pattern: regex::Regex,
}

impl ParameterizedQueryHandler {
    /// Create a new parametric query processor.
    pub fn new() -> Self {
        Self {
            placeholder_pattern: regex::Regex::new(r"\$(\d+|[a-zA-Z_][a-zA-Z0-9_]*)")
                .expect("Placeholder regex compilation failed"),
        }
    }

    /// Extract the parameter positions from the query.
    ///
    /// # Parameters
    /// - `query`: query text
    ///
    /// # Returns
    /// Parameter Location List
    pub fn extract_params(&self, query: &str) -> Vec<ParamPosition> {
        self.extract_param_matches(query)
            .into_iter()
            .map(|(position, _)| position)
            .collect()
    }

    /// Extract the parameter matches from the query together with their end
    /// offsets. Assignment left-hand sides (`$var = ...`) are excluded.
    fn extract_param_matches(&self, query: &str) -> Vec<(ParamPosition, usize)> {
        let mut positions = Vec::new();

        for (idx, cap) in self.placeholder_pattern.captures_iter(query).enumerate() {
            let full_match = cap.get(0).expect("Full match should exist");
            let param_str = cap.get(1).expect("Parameter group should exist").as_str();

            // Skip the left-hand side of a variable assignment, e.g.
            // `$result = GO ...` defines a session variable instead of
            // declaring a named query parameter.
            let after_match = &query[full_match.end()..];
            if after_match.trim_start().starts_with('=') {
                continue;
            }

            let (index, name) = if param_str.chars().all(|c| c.is_ascii_digit()) {
                (param_str.parse::<usize>().unwrap_or(idx), None)
            } else {
                (idx, Some(param_str.to_string()))
            };

            positions.push((
                ParamPosition {
                    index,
                    name,
                    position: full_match.start(),
                    expected_type: None,
                },
                full_match.end(),
            ));
        }

        positions
    }

    /// Parameterize the query (replace parameters with placeholders)
    ///
    /// # Parameters
    /// - `query`: query text
    ///
    /// # Returns
    /// (parameterized query, parameter list)
    pub fn parameterize(&self, query: &str) -> (String, Vec<ParamPosition>) {
        let matches = self.extract_param_matches(query);
        let positions = matches
            .iter()
            .map(|(position, _)| position.clone())
            .collect::<Vec<_>>();

        // Replace only the matches that were accepted as parameters so that
        // assignment left-hand sides ($var = ...) stay intact in the template.
        let mut parameterized = String::with_capacity(query.len());
        let mut last_end = 0;
        for (position, end) in matches {
            parameterized.push_str(&query[last_end..position.position]);
            parameterized.push('?');
            last_end = end;
        }
        parameterized.push_str(&query[last_end..]);
        (parameterized, positions)
    }
}

impl Default for ParameterizedQueryHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_cache_key() {
        let key1 = PlanCacheKey::from_query("SELECT * FROM users");
        let key2 = PlanCacheKey::from_query("SELECT * FROM users");
        let key3 = PlanCacheKey::from_query("SELECT * FROM posts");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_plan_cache_key_verify() {
        let key = PlanCacheKey::from_query("SELECT * FROM users");
        assert!(key.verify_query("SELECT * FROM users"));
        assert!(!key.verify_query("SELECT * FROM posts"));
    }

    #[test]
    fn cache_key_covers_all_compatibility_dimensions() {
        let base = PlanCacheContext {
            space_name: Some("space".to_string()),
            schema_version: Some(1),
            index_version: Some(2),
            param_type_signature: Some(3),
            optimizer_version: 4,
            planning_config_hash: 5,
            capability_set: 6,
        };
        let key = PlanCacheKey::from_query_with_context("MATCH (n) RETURN n", base.clone());
        for changed in [
            PlanCacheContext {
                optimizer_version: 7,
                ..base.clone()
            },
            PlanCacheContext {
                planning_config_hash: 7,
                ..base.clone()
            },
            PlanCacheContext {
                capability_set: 7,
                ..base.clone()
            },
            PlanCacheContext {
                schema_version: Some(7),
                ..base.clone()
            },
            PlanCacheContext {
                index_version: Some(7),
                ..base.clone()
            },
        ] {
            assert_ne!(
                key,
                PlanCacheKey::from_query_with_context("MATCH (n) RETURN n", changed)
            );
        }
    }

    #[test]
    fn test_parameterized_query_handler() {
        let handler = ParameterizedQueryHandler::new();

        let params = handler.extract_params("SELECT * FROM users WHERE id = $1 AND name = $name");

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].index, 1);
        assert!(params[0].name.is_none());
        assert_eq!(params[1].index, 1);
        assert_eq!(params[1].name, Some("name".to_string()));
    }

    #[test]
    fn test_parameterized_query_handler_parameterize() {
        let handler = ParameterizedQueryHandler::new();

        let (parameterized, params) = handler.parameterize("SELECT * FROM users WHERE id = $1");

        assert_eq!(parameterized, "SELECT * FROM users WHERE id = ?");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_query_plan_cache_basic() {
        let cache = QueryPlanCache::default();

        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_priority_ordering() {
        assert!(CachePriority::Critical > CachePriority::High);
        assert!(CachePriority::High > CachePriority::Normal);
        assert!(CachePriority::Normal > CachePriority::Low);
    }

    fn make_spec(ranges: &[(i64, i64)], layout_version: Option<u64>) -> PartitionSpec {
        PartitionSpec::try_new(
            ranges
                .iter()
                .map(|(start, end)| *start..*end)
                .collect(),
            PartitionSource::VertexId {
                tag: "Node".to_string(),
            },
            layout_version,
        )
        .expect("valid spec")
    }

    #[test]
    fn partition_fingerprint_changes_with_ranges() {
        let key_a = PlanCacheKey::from_query_with_partition(
            "MATCH (n:Node) RETURN n",
            &make_spec(&[(0, 100), (100, 200)], Some(1)),
        );
        let key_b = PlanCacheKey::from_query_with_partition(
            "MATCH (n:Node) RETURN n",
            &make_spec(&[(0, 50), (50, 100), (100, 200)], Some(1)),
        );
        assert_ne!(key_a, key_b, "different range splits must not share a key");
    }

    #[test]
    fn partition_fingerprint_changes_with_source_and_layout_version() {
        let base = make_spec(&[(0, 100)], Some(1));
        let other_source = PartitionSpec::try_new(
            vec![0..100],
            PartitionSource::EdgeId {
                edge_type: "Link".to_string(),
            },
            Some(1),
        )
        .expect("valid spec");
        let bumped_version = make_spec(&[(0, 100)], Some(2));

        let key_base =
            PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &base);
        assert_ne!(
            key_base,
            PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &other_source),
            "different data domains must not share a key"
        );
        assert_ne!(
            key_base,
            PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &bumped_version),
            "layout version bumps must not share a key"
        );
    }

    #[test]
    fn partitioned_plan_is_isolated_from_plain_text_lookup() {
        use crate::query::executor::streaming::plan::types::{
            CapabilitySet, FragmentGraph, FragmentId, OutputContract, PlanCompatibility,
            PlanFingerprint, PipelineMode,
        };
        use crate::query::executor::streaming::parameters::ParameterSchema;
        use crate::query::executor::streaming::slot::SlotLayout;
        use std::collections::HashMap;

        let cache = QueryPlanCache::default();
        let query = "MATCH (n:Node) RETURN n";
        let spec = make_spec(&[(0, 100)], Some(1));
        let plan = Arc::new(PhysicalPlan {
            operators: Vec::new(),
            logical_to_physical: HashMap::new(),
            fragments: FragmentGraph::new(Vec::new(), FragmentId(0)),
            root_fragment: FragmentId(0),
            output: OutputContract {
                output_layout: SlotLayout::new(Vec::new()),
                always_produces_row: false,
                nullability: Vec::new(),
                ordering: Vec::new(),
                delivery_streamable: true,
                pipeline_mode: PipelineMode::Pipelined,
            },
            compatibility: PlanCompatibility {
                fingerprint: PlanFingerprint { version: 1, hash: 0 },
                layout_version: None,
                required_capabilities: CapabilitySet::EMPTY,
                planning_config_hash: 0,
                optimizer_version: 0,
            },
            required_capabilities: CapabilitySet::EMPTY,
            parameter_schema: ParameterSchema {
                params: Vec::new(),
                name_to_slot: HashMap::new(),
            },
            parallel_fallback_reason: String::new(),
            partition_spec: Some(spec.clone()),
        });

        cache.put_with_partition(query, &spec, plan.clone(), Vec::new());
        assert!(
            cache.get(query).is_none(),
            "plain-text lookup must not serve a partitioned plan"
        );
        let cached = cache
            .get_with_partition(query, &spec)
            .expect("partition-keyed lookup should hit");
        assert_eq!(cached.plan.fragment_count(), 0);

        let other = make_spec(&[(0, 50), (50, 100)], Some(1));
        assert!(
            cache.get_with_partition(query, &other).is_none(),
            "a different layout must not hit the cached partitioned plan"
        );
    }
}
