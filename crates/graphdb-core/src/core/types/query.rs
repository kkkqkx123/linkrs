//! Query Type Base Definition
//!
//! This module defines types related to query processing, including query types,
//! execution options, and query statistics.

use serde::{Deserialize, Serialize};

/// Enumeration of query types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum QueryType {
    #[default]
    ReadQuery,
    WriteQuery,
    AdminQuery,
    SchemaQuery,
}

/// Query execution options
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryOptions {
    /// Maximum number of rows to return
    pub limit: Option<usize>,
    /// Query timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Whether to use cache
    pub use_cache: bool,
    /// Maximum recursion depth for recursive queries
    pub max_recursion_depth: Option<u32>,
    /// Whether to enable profiling
    pub enable_profiling: bool,
    /// Query priority (higher value = higher priority)
    pub priority: u8,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            limit: None,
            timeout_ms: None,
            use_cache: true,
            max_recursion_depth: Some(100),
            enable_profiling: false,
            priority: 5,
        }
    }
}

impl QueryOptions {
    /// Create a new query options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Set whether to use cache
    pub fn with_cache(mut self, use_cache: bool) -> Self {
        self.use_cache = use_cache;
        self
    }

    /// Set the maximum recursion depth
    pub fn with_max_recursion_depth(mut self, depth: u32) -> Self {
        self.max_recursion_depth = Some(depth);
        self
    }

    /// Set whether to enable profiling
    pub fn with_profiling(mut self, enable: bool) -> Self {
        self.enable_profiling = enable;
        self
    }

    /// Set the query priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// Query execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryStatus {
    /// Query is pending execution
    Pending,
    /// Query is currently executing
    Executing,
    /// Query completed successfully
    Completed,
    /// Query failed with an error
    Failed,
    /// Query was cancelled
    Cancelled,
    /// Query timed out
    Timeout,
}

impl QueryStatus {
    /// Check if the query is in a final state
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            QueryStatus::Completed
                | QueryStatus::Failed
                | QueryStatus::Cancelled
                | QueryStatus::Timeout
        )
    }

    /// Check if the query completed successfully
    pub fn is_success(&self) -> bool {
        matches!(self, QueryStatus::Completed)
    }

    /// Get the status name
    pub fn name(&self) -> &'static str {
        match self {
            QueryStatus::Pending => "PENDING",
            QueryStatus::Executing => "EXECUTING",
            QueryStatus::Completed => "COMPLETED",
            QueryStatus::Failed => "FAILED",
            QueryStatus::Cancelled => "CANCELLED",
            QueryStatus::Timeout => "TIMEOUT",
        }
    }
}

/// Query execution statistics
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QueryStats {
    /// Total execution time in milliseconds
    pub execution_time_ms: u64,
    /// Time spent in planning phase
    pub planning_time_ms: u64,
    /// Time spent in execution phase
    pub query_time_ms: u64,
    /// Number of rows returned
    pub rows_returned: usize,
    /// Number of rows scanned
    pub rows_scanned: usize,
    /// Number of index hits
    pub index_hits: usize,
    /// Memory usage in bytes
    pub memory_usage: usize,
    /// Number of cache hits
    pub cache_hits: usize,
    /// Number of cache misses
    pub cache_misses: usize,
}

impl QueryStats {
    /// Create a new query stats
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate cache hit ratio
    pub fn cache_hit_ratio(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    /// Get total cache accesses
    pub fn total_cache_accesses(&self) -> usize {
        self.cache_hits + self.cache_misses
    }
}

/// Query plan type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanType {
    /// Sequential scan plan
    SequentialScan,
    /// Index scan plan
    IndexScan,
    /// Index seek plan
    IndexSeek,
    /// Join plan
    Join,
    /// Aggregation plan
    Aggregation,
    /// Sort plan
    Sort,
    /// Limit plan
    Limit,
    /// Filter plan
    Filter,
    /// Projection plan
    Projection,
    /// Union plan
    Union,
    /// Intersection plan
    Intersection,
    /// Graph traversal plan
    GraphTraversal,
}

impl PlanType {
    /// Get the plan type name
    pub fn name(&self) -> &'static str {
        match self {
            PlanType::SequentialScan => "SEQUENTIAL_SCAN",
            PlanType::IndexScan => "INDEX_SCAN",
            PlanType::IndexSeek => "INDEX_SEEK",
            PlanType::Join => "JOIN",
            PlanType::Aggregation => "AGGREGATION",
            PlanType::Sort => "SORT",
            PlanType::Limit => "LIMIT",
            PlanType::Filter => "FILTER",
            PlanType::Projection => "PROJECTION",
            PlanType::Union => "UNION",
            PlanType::Intersection => "INTERSECTION",
            PlanType::GraphTraversal => "GRAPH_TRAVERSAL",
        }
    }

    /// Check if this plan type uses index
    pub fn uses_index(&self) -> bool {
        matches!(self, PlanType::IndexScan | PlanType::IndexSeek)
    }

    /// Check if this plan type is a scan operation
    pub fn is_scan(&self) -> bool {
        matches!(
            self,
            PlanType::SequentialScan | PlanType::IndexScan | PlanType::IndexSeek
        )
    }
}

/// Query hint type for optimizer hints
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QueryHint {
    /// Force using a specific index
    UseIndex(String),
    /// Force not using any index
    NoIndex,
    /// Set join order
    JoinOrder(Vec<String>),
    /// Set parallelism degree
    Parallelism(u32),
    /// Force a specific plan type
    ForcePlan(PlanType),
    /// Set batch size
    BatchSize(usize),
}

/// Query execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExecutionMode {
    /// Execute in auto mode (let optimizer decide)
    #[default]
    Auto,
    /// Force sequential execution
    Sequential,
    /// Force parallel execution
    Parallel,
    /// Distributed execution (reserved for future)
    Distributed,
}

impl ExecutionMode {
    /// Get the execution mode name
    pub fn name(&self) -> &'static str {
        match self {
            ExecutionMode::Auto => "AUTO",
            ExecutionMode::Sequential => "SEQUENTIAL",
            ExecutionMode::Parallel => "PARALLEL",
            ExecutionMode::Distributed => "DISTRIBUTED",
        }
    }

    /// Check if parallel execution is allowed
    pub fn allows_parallel(&self) -> bool {
        matches!(self, ExecutionMode::Auto | ExecutionMode::Parallel)
    }
}
