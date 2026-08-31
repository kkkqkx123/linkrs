use crate::cache::config::CachePriority;
use crate::cache::plan_cache::key::ParamPosition;
use crate::executor::streaming::plan::PhysicalPlan;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
