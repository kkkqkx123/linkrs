//! Physical partition layout selected for a plan.
//!
//! Extracted from `execution_plan.rs` so that downstream consumers (the
//! arena `PhysicalPlan`, the plan cache, the partitioned arena builder, the
//! streaming partition view) no longer depend on the legacy `ExecutionPlan`
//! module just to reach `PartitionSpec`.  The optimizer and cache — which
//! sit upstream of the executor — also import from here, keeping the
//! dependency direction flat (`planning::plan::partition_spec` has no
//! intra-crate dependencies beyond `core`).

use std::ops::Range;
use std::{error::Error, fmt};

/// Identifies the data domain that a partition layout maps ranges over.
///
/// This prevents the plan cache from reusing a stale `PartitionSpec` when the
/// underlying storage layout has changed (e.g. re-indexing, new vertex tag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionSource {
    /// Ranges over a vertex-id space identified by a tag.
    VertexId { tag: String },
    /// Ranges over an edge-id space identified by an edge type.
    EdgeId { edge_type: String },
    /// Ranges over an explicit index's key space.
    Index { index_name: String },
}

impl fmt::Display for PartitionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VertexId { tag } => write!(formatter, "vertex tag '{tag}'"),
            Self::EdgeId { edge_type } => write!(formatter, "edge type '{edge_type}'"),
            Self::Index { index_name } => write!(formatter, "index '{index_name}'"),
        }
    }
}

/// Physical partition layout selected for a plan.
///
/// An absent layout means single-tree execution.  The planner must only set a
/// layout after it has split the logical plan at a valid exchange boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionSpec {
    ranges: Vec<Range<i64>>,
    /// Data domain these ranges map onto.
    source: PartitionSource,
    /// Monotonically-increasing layout version.  When the underlying data
    /// layout changes this version lets the plan cache detect stale specs.
    layout_version: Option<u64>,
}

/// Validation error returned when a physical partition layout cannot be
/// executed safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionSpecError {
    Empty,
    EmptyRange { index: usize },
    UnorderedOrOverlapping { index: usize },
}

impl fmt::Display for PartitionSpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(
                formatter,
                "partition layout must contain at least one range"
            ),
            Self::EmptyRange { index } => {
                write!(
                    formatter,
                    "partition range at index {index} must not be empty"
                )
            }
            Self::UnorderedOrOverlapping { index } => write!(
                formatter,
                "partition range at index {index} must be ordered and non-overlapping"
            ),
        }
    }
}

impl Error for PartitionSpecError {}

impl PartitionSpec {
    /// Create a validated physical partition layout.
    ///
    /// Ranges are ordered by start and may be disjoint, but they must not be
    /// empty or overlap. Keeping this invariant at the plan boundary prevents
    /// duplicated or missing rows once a scan is copied for each partition.
    pub fn try_new(
        ranges: Vec<Range<i64>>,
        source: PartitionSource,
        layout_version: Option<u64>,
    ) -> Result<Self, PartitionSpecError> {
        if ranges.is_empty() {
            return Err(PartitionSpecError::Empty);
        }

        let mut previous_end = None;
        for (index, range) in ranges.iter().enumerate() {
            if range.start >= range.end {
                return Err(PartitionSpecError::EmptyRange { index });
            }
            if previous_end.is_some_and(|end| range.start < end) {
                return Err(PartitionSpecError::UnorderedOrOverlapping { index });
            }
            previous_end = Some(range.end);
        }

        Ok(Self {
            ranges,
            source,
            layout_version,
        })
    }

    pub fn ranges(&self) -> &[Range<i64>] {
        &self.ranges
    }

    pub fn partition_count(&self) -> usize {
        self.ranges.len()
    }

    pub fn source(&self) -> &PartitionSource {
        &self.source
    }

    pub fn layout_version(&self) -> Option<u64> {
        self.layout_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source() -> PartitionSource {
        PartitionSource::VertexId {
            tag: "test".to_string(),
        }
    }

    #[test]
    fn partition_spec_rejects_empty_and_overlapping_ranges() {
        assert_eq!(
            PartitionSpec::try_new(Vec::new(), test_source(), None),
            Err(PartitionSpecError::Empty)
        );
        assert_eq!(
            PartitionSpec::try_new(std::iter::once(0..0).collect(), test_source(), None),
            Err(PartitionSpecError::EmptyRange { index: 0 })
        );
        assert_eq!(
            PartitionSpec::try_new(vec![0..10, 5..20], test_source(), None),
            Err(PartitionSpecError::UnorderedOrOverlapping { index: 1 })
        );
    }

    #[test]
    fn partition_spec_stores_source_and_layout_version() {
        let spec = PartitionSpec::try_new(vec![0..10, 10..20], test_source(), Some(42))
            .expect("valid spec");
        assert_eq!(spec.source(), &test_source());
        assert_eq!(spec.layout_version(), Some(42));
        assert_eq!(spec.partition_count(), 2);
    }
}