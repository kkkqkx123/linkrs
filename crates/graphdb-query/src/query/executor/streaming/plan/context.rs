//! PhysicalPlanBuildContext: read-only context for building physical plans.
//!
//! Contains only schema catalog, statistics, capability information, and
//! planning configuration — never runtime handles, storage clients,
//! transaction handles, or per-query mutable state.
//!
//! This ensures the resulting [`PhysicalPlan`](super::types::PhysicalPlan)
//! is immutable, cacheable, and safe to share across concurrent executions.

use super::types::{FragmentIdAllocator, PhysicalOperatorIdAllocator};
use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::parameters::ParameterSchema;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::planning::plan::PartitionSpec;

/// Schema identifier for catalog lookups during plan building.
#[derive(Debug, Clone)]
pub struct SchemaRef {
    pub space_name: String,
    pub layout_version: u64,
}

/// Snapshot of table/index statistics used for cardinality estimation.
#[derive(Debug, Clone, Default)]
pub struct StatisticsSnapshot {
    pub row_count_estimates: Vec<(String, u64)>,
}

/// Planning configuration flags and thresholds.
#[derive(Debug, Clone)]
pub struct PlanningConfig {
    /// Maximum number of partitions to generate.
    pub max_partitions: usize,
    /// Whether to enable hash join.
    pub enable_hash_join: bool,
    /// Optimizer rule set version.
    pub optimizer_version: u64,
    /// Hash of this config for cache compatibility.
    pub config_hash: u64,
}

impl Default for PlanningConfig {
    fn default() -> Self {
        Self {
            max_partitions: 4,
            enable_hash_join: true,
            optimizer_version: 1,
            config_hash: 0,
        }
    }
}

/// Read-only context used during [`PhysicalPlan`] construction.
///
/// Intentionally free of:
/// - `QueryStorage`, transaction/session handles
/// - Runtime, memory tracker, cursor, buffer, emitted state
/// - Per-execution parameter values, auth context, current snapshot
/// - Temporary space/storage references that belong in bindings
#[derive(Debug, Clone)]
pub struct PhysicalPlanBuildContext {
    /// Schema catalog reference (space schema + layout version).
    pub schema: Option<SchemaRef>,
    /// Statistics snapshot for cardinality estimation.
    pub statistics: StatisticsSnapshot,
    /// Planning configuration.
    pub config: PlanningConfig,
    /// The output slot layout expected by the parent (if known at build time).
    pub expected_output_layout: Option<SlotLayout>,

    /// Parameter schema for prepared-statement parameters.
    pub parameter_schema: ParameterSchema,

    /// M3: optional partition spec for partitioned execution.
    /// When set, the builder produces a PhysicalPlan with multiple source
    /// fragments and exchange/gather fragments instead of a single linear chain.
    pub partition_spec: Option<PartitionSpec>,

    /// Why parallel partitioning was not applied (empty = partitioning
    /// active or not requested). Copied into the built [`PhysicalPlan`] so
    /// EXPLAIN / PROFILE diagnostics can surface it.
    pub parallel_fallback_reason: String,

    // ── Allocators ──
    pub(crate) operator_id_alloc: PhysicalOperatorIdAllocator,
    pub(crate) fragment_id_alloc: FragmentIdAllocator,
    // NOTE: capacity planning and spill config will be added in P2.
}

impl PhysicalPlanBuildContext {
    /// Create a new build context from an [`ExecutionContext`], extracting
    /// only the immutable, plan-relevant portions.
    ///
    /// Note: `ExecutionContext` still carries runtime-bound state that
    /// should eventually be separated.  This constructor extracts what it
    /// needs and leaves the rest behind.
    pub fn from_execution_context(context: &ExecutionContext) -> Self {
        Self {
            schema: context.space_name.as_ref().map(|space_name| SchemaRef {
                space_name: space_name.clone(),
                layout_version: 0,
            }),
            statistics: StatisticsSnapshot::default(),
            config: PlanningConfig {
                max_partitions: context.max_workers,
                ..PlanningConfig::default()
            },
            expected_output_layout: None,
            parameter_schema: ParameterSchema::default(),
            partition_spec: None,
            parallel_fallback_reason: String::new(),
            operator_id_alloc: PhysicalOperatorIdAllocator::new(),
            fragment_id_alloc: FragmentIdAllocator::new(),
        }
    }

    /// Create a minimal context for testing or simple scans.
    pub fn new() -> Self {
        Self {
            schema: None,
            statistics: StatisticsSnapshot::default(),
            config: PlanningConfig::default(),
            expected_output_layout: None,
            parameter_schema: ParameterSchema::default(),
            partition_spec: None,
            parallel_fallback_reason: String::new(),
            operator_id_alloc: PhysicalOperatorIdAllocator::new(),
            fragment_id_alloc: FragmentIdAllocator::new(),
        }
    }

    /// Allocate a new physical operator ID from the unified arena.
    pub fn allocate_operator_id(
        &mut self,
    ) -> crate::query::executor::streaming::plan::types::PhysicalOperatorId {
        self.operator_id_alloc.allocate()
    }

    /// Allocate a new fragment ID.
    pub fn allocate_fragment_id(
        &mut self,
    ) -> crate::query::executor::streaming::plan::types::FragmentId {
        self.fragment_id_alloc.allocate()
    }

    /// Peek at the next operator ID without consuming it.
    pub fn peek_operator_id(
        &self,
    ) -> crate::query::executor::streaming::plan::types::PhysicalOperatorId {
        self.operator_id_alloc.peek()
    }

    /// Check that this context has been configured correctly.
    pub fn validate(&self) -> Result<(), QueryError> {
        Ok(())
    }
}

impl Default for PhysicalPlanBuildContext {
    fn default() -> Self {
        Self::new()
    }
}
