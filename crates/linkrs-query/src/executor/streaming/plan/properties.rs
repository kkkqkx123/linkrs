//! Physical properties for physical plan nodes.
//!
//! Describes the output characteristics of each physical operator:
//! distribution, ordering, pipeline kind, parallelism, and memory policy.
//! Used in the cost model, optimizer, and parallel execution planning.

use super::super::slot::SlotId;

/// Data distribution strategy for the output of a physical node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distribution {
    Single,
    Broadcast,
    HashPartitioned(Vec<SlotId>),
}

/// Output ordering guarantee.
#[derive(Debug, Clone)]
pub enum Ordering {
    None,
    Sorted(Vec<SortOrder>),
}

#[derive(Debug, Clone)]
pub struct SortOrder {
    pub slot: SlotId,
    pub ascending: bool,
}

/// Whether the operator is streaming (produces incremental results)
/// or blocking (must consume all input before producing output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineKind {
    Streaming,
    Blocking,
}

/// Parallelism constraints for this node.
#[derive(Debug, Clone)]
pub struct Parallelism {
    pub min_workers: usize,
    pub max_workers: usize,
}

impl Default for Parallelism {
    fn default() -> Self {
        Self {
            min_workers: 1,
            max_workers: 1,
        }
    }
}

/// Memory policy for blocking/spill-capable operators.
///
/// - `None`: no memory tracking (streaming operators that don't accumulate).
/// - `RequiresBudget`: operator tracks memory via `MemoryTracker` but does NOT
///   spill. When budget is exceeded, returns `ResourceExhausted`.
/// - `Spillable`: operator can spill to disk when budget is exceeded.
///   `threshold` is a plan-level trigger hint in bytes.
#[derive(Debug, Clone, Default)]
pub enum MemoryPolicy {
    #[default]
    None,
    RequiresBudget,
    Spillable {
        threshold: u64,
    },
}

/// Default spill threshold for operators with full external spill support.
pub const SPILL_DEFAULT_THRESHOLD: u64 = 64 * 1024 * 1024; // 64 MB

/// Physical plan properties attached to each node's output.
#[derive(Debug, Clone)]
pub struct PhysicalProperties {
    pub distribution: Distribution,
    pub ordering: Ordering,
    pub pipeline_kind: PipelineKind,
    pub parallelism: Parallelism,
    pub memory_policy: MemoryPolicy,
}

impl PhysicalProperties {
    pub fn new(
        distribution: Distribution,
        ordering: Ordering,
        pipeline_kind: PipelineKind,
        parallelism: Parallelism,
        memory_policy: MemoryPolicy,
    ) -> Self {
        Self {
            distribution,
            ordering,
            pipeline_kind,
            parallelism,
            memory_policy,
        }
    }

    pub fn single_streaming() -> Self {
        Self::new(
            Distribution::Single,
            Ordering::None,
            PipelineKind::Streaming,
            Parallelism::default(),
            MemoryPolicy::None,
        )
    }

    pub fn single_blocking() -> Self {
        Self::new(
            Distribution::Single,
            Ordering::None,
            PipelineKind::Blocking,
            Parallelism::default(),
            MemoryPolicy::None,
        )
    }

    /// Blocking operator that tracks memory but does not spill.
    pub fn single_blocking_with_budget() -> Self {
        Self::new(
            Distribution::Single,
            Ordering::None,
            PipelineKind::Blocking,
            Parallelism::default(),
            MemoryPolicy::RequiresBudget,
        )
    }

    /// Blocking operator with full external spill support.
    pub fn single_blocking_spillable(threshold: u64) -> Self {
        Self::new(
            Distribution::Single,
            Ordering::None,
            PipelineKind::Blocking,
            Parallelism::default(),
            MemoryPolicy::Spillable { threshold },
        )
    }

    pub fn sorted_blocking(ordering: Ordering) -> Self {
        Self::new(
            Distribution::Single,
            ordering,
            PipelineKind::Blocking,
            Parallelism::default(),
            MemoryPolicy::None,
        )
    }
}
