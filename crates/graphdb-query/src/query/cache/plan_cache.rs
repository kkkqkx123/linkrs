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
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::stats::StatsManager;
use crate::query::planning::plan::ExecutionPlan;

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

/// Query Plan Cache Key
///
/// Supports fast lookups using the hash of the query text as the key.
/// Also store query text for conflict detection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanCacheKey {
    /// Query the hash value of the text
    pub hash: u64,
    /// Query text (for conflict detection, not just debugging)
    query_text: String,
}

impl PlanCacheKey {
    /// Creating Cache Keys from Query Text
    pub fn from_query(query: &str) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            hash,
            query_text: query.to_string(),
        }
    }

    /// Verify that the query text matches (for conflict detection)
    pub fn verify_query(&self, query: &str) -> bool {
        self.query_text == query
    }

    /// Get query text (for debugging or logging)
    pub fn query_text(&self) -> &str {
        &self.query_text
    }
}

/// Cached query plan entries
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// Query template (parameterized form)
    pub query_template: String,
    /// implementation plan
    pub plan: ExecutionPlan,
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

    /// Estimate memory usage for execution plan
    fn estimate_plan_memory(&self, plan: &ExecutionPlan) -> usize {
        let base_size = std::mem::size_of::<ExecutionPlan>();
        let format_size = plan.format.len();

        let root_size = if let Some(ref root) = plan.root {
            self.estimate_node_memory(root)
        } else {
            0
        };

        base_size + format_size + root_size
    }

    /// Estimate memory usage for plan node
    fn estimate_node_memory(&self, node: &crate::query::planning::plan::PlanNodeEnum) -> usize {
        let base_size = std::mem::size_of::<crate::query::planning::plan::PlanNodeEnum>();

        let children_size: usize = node
            .children()
            .iter()
            .map(|child| self.estimate_node_memory(child))
            .sum();

        base_size + children_size
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
        let key = PlanCacheKey::from_query(query);

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
    /// # Parameters
    /// - `query`: Query text
    /// - `plan`: Execution plan
    /// - `param_positions`: Information about the positions of the parameters
    pub fn put(&self, query: &str, plan: ExecutionPlan, param_positions: Vec<ParamPosition>) {
        self.put_with_tables(query, plan, param_positions, Vec::new());
    }

    /// Put the plan in the cache with dependent tables.
    pub fn put_with_tables(
        &self,
        query: &str,
        plan: ExecutionPlan,
        param_positions: Vec<ParamPosition>,
        dependent_tables: Vec<String>,
    ) {
        let key = PlanCacheKey::from_query(query);
        let query_bytes = query.len();

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

    /// Calculate priority based on query characteristics
    fn calculate_priority(&self, plan: &ExecutionPlan) -> CachePriority {
        let complexity = self.calculate_complexity_score(plan);

        if complexity > 1000 {
            CachePriority::High
        } else if complexity > 100 {
            CachePriority::Normal
        } else {
            CachePriority::Low
        }
    }

    /// Calculate complexity score for a plan based on actual plan structure
    fn calculate_complexity_score(&self, plan: &ExecutionPlan) -> u32 {
        let mut score = 0u32;

        if let Some(ref root) = plan.root {
            score += self.node_complexity_score(root);
        }

        score += (plan.format.len() / 100) as u32;

        score
    }

    /// Calculate complexity score for a plan node
    fn node_complexity_score(&self, node: &crate::query::planning::plan::PlanNodeEnum) -> u32 {
        use crate::query::planning::plan::PlanNodeEnum;

        let mut score = match node {
            // Access nodes
            PlanNodeEnum::Start(_) => 0,
            PlanNodeEnum::GetVertices(_) => 10,
            PlanNodeEnum::GetEdges(_) => 10,
            PlanNodeEnum::GetNeighbors(_) => 15,
            PlanNodeEnum::ScanVertices(_) => 15,
            PlanNodeEnum::ScanEdges(_) => 15,
            PlanNodeEnum::EdgeIndexScan(_) => 20,
            PlanNodeEnum::IndexScan(_) => 20,

            // Operation nodes
            PlanNodeEnum::Project(_) => 10,
            PlanNodeEnum::Filter(_) => 20,
            PlanNodeEnum::Sort(_) => 30,
            PlanNodeEnum::Limit(_) => 5,
            PlanNodeEnum::TopN(_) => 35,
            PlanNodeEnum::Sample(_) => 15,
            PlanNodeEnum::Dedup(_) => 20,
            PlanNodeEnum::Aggregate(_) => 40,

            // Join nodes
            PlanNodeEnum::InnerJoin(_) => 50,
            PlanNodeEnum::LeftJoin(_) => 50,
            PlanNodeEnum::RightJoin(_) => 50,
            PlanNodeEnum::CrossJoin(_) => 45,
            PlanNodeEnum::HashInnerJoin(_) => 55,
            PlanNodeEnum::HashLeftJoin(_) => 55,
            PlanNodeEnum::FullOuterJoin(_) => 60,
            PlanNodeEnum::SemiJoin(_) => 40,

            // Traversal nodes
            PlanNodeEnum::Expand(_) => 25,
            PlanNodeEnum::ExpandAll(_) => 30,
            PlanNodeEnum::Traverse(_) => 35,
            PlanNodeEnum::AppendVertices(_) => 20,
            PlanNodeEnum::BiExpand(_) => 30,
            PlanNodeEnum::BiTraverse(_) => 35,

            // Control flow nodes
            PlanNodeEnum::Argument(_) => 5,
            PlanNodeEnum::Loop(_) => 50,
            PlanNodeEnum::PassThrough(_) => 5,
            PlanNodeEnum::Select(_) => 25,

            // Transaction nodes
            PlanNodeEnum::BeginTransaction(_) => 10,
            PlanNodeEnum::Commit(_) => 10,
            PlanNodeEnum::Rollback(_) => 10,

            // Data processing nodes
            PlanNodeEnum::DataCollect(_) => 15,
            PlanNodeEnum::Remove(_) => 15,
            PlanNodeEnum::PatternApply(_) => 35,
            PlanNodeEnum::RollUpApply(_) => 35,
            PlanNodeEnum::Union(_) => 25,
            PlanNodeEnum::Minus(_) => 30,
            PlanNodeEnum::Intersect(_) => 30,
            PlanNodeEnum::Unwind(_) => 15,
            PlanNodeEnum::Materialize(_) => 20,
            PlanNodeEnum::Assign(_) => 10,
            PlanNodeEnum::Apply(_) => 30,

            // Algorithm nodes
            PlanNodeEnum::MultiShortestPath(_) => 60,
            PlanNodeEnum::BFSShortest(_) => 50,
            PlanNodeEnum::AllPaths(_) => 55,
            PlanNodeEnum::ShortestPath(_) => 55,

            // Management nodes
            PlanNodeEnum::SpaceManage(_) => 10,
            PlanNodeEnum::TagManage(_) => 10,
            PlanNodeEnum::EdgeManage(_) => 10,
            PlanNodeEnum::IndexManage(_) => 10,
            PlanNodeEnum::UserManage(_) => 10,
            PlanNodeEnum::FulltextManage(_) => 10,
            PlanNodeEnum::VectorManage(_) => 10,

            // Data modification nodes
            PlanNodeEnum::InsertVertices(_) => 25,
            PlanNodeEnum::InsertEdges(_) => 25,
            PlanNodeEnum::DeleteVertices(_) => 20,
            PlanNodeEnum::DeleteEdges(_) => 20,
            PlanNodeEnum::DeleteTags(_) => 20,
            PlanNodeEnum::DeleteIndex(_) => 15,
            PlanNodeEnum::PipeDeleteVertices(_) => 25,
            PlanNodeEnum::PipeDeleteEdges(_) => 25,
            PlanNodeEnum::Update(_) => 25,
            PlanNodeEnum::UpdateVertices(_) => 25,
            PlanNodeEnum::UpdateEdges(_) => 25,

            // Stats nodes
            PlanNodeEnum::ShowStats(_) => 10,

            // Full-text search nodes
            PlanNodeEnum::FulltextSearch(_) => 30,
            PlanNodeEnum::FulltextLookup(_) => 25,
            PlanNodeEnum::MatchFulltext(_) => 30,

            // Vector search nodes
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(_) => 35,
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorLookup(_) => 30,
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorMatch(_) => 35,
        };

        for child in node.children() {
            score += self.node_complexity_score(child);
        }

        score
    }

    /// Estimate compute cost in milliseconds
    fn estimate_compute_cost(&self, plan: &ExecutionPlan) -> u64 {
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
        let mut positions = Vec::new();

        for (idx, cap) in self.placeholder_pattern.captures_iter(query).enumerate() {
            let full_match = cap.get(0).expect("Full match should exist");
            let param_str = cap.get(1).expect("Parameter group should exist").as_str();

            let (index, name) = if param_str.chars().all(|c| c.is_ascii_digit()) {
                (param_str.parse::<usize>().unwrap_or(idx), None)
            } else {
                (idx, Some(param_str.to_string()))
            };

            positions.push(ParamPosition {
                index,
                name,
                position: full_match.start(),
                expected_type: None,
            });
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
        let positions = self.extract_params(query);
        let parameterized = self.placeholder_pattern.replace_all(query, "?").to_string();

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
}
