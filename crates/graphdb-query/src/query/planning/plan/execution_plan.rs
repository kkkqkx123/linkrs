//! Structure definition of the execution plan
//! Contains the ExecutionPlan and SubPlan structures.

use std::ops::Range;
use std::{error::Error, fmt};

use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Execution mode for a query plan
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    /// Use traditional materialized execution (buffer all intermediate results)
    #[default]
    Materialized,
    /// Use streaming pull-based execution (process row-at-a-time, minimal buffering)
    Streaming,
}

impl ExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionMode::Materialized => "Materialized",
            ExecutionMode::Streaming => "Streaming",
        }
    }
}

/// Physical partition layout selected for a plan.
///
/// An absent layout means single-tree execution.  The planner must only set a
/// layout after it has split the logical plan at a valid exchange boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionSpec {
    ranges: Vec<Range<u32>>,
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
    pub fn try_new(ranges: Vec<Range<u32>>) -> Result<Self, PartitionSpecError> {
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

        Ok(Self { ranges })
    }

    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    pub fn partition_count(&self) -> usize {
        self.ranges.len()
    }
}

/// A physical decomposition of a logical plan at partition exchange
/// boundaries. Logical nodes are retained verbatim; this type only describes
/// where they run (local partition trees or one global tree).
#[derive(Debug, Clone)]
pub struct PartitionedPhysicalPlan {
    partition_spec: PartitionSpec,
    root: PartitionedPhysicalNode,
}

#[derive(Debug, Clone)]
pub enum PartitionedPhysicalNode {
    /// A subtree that can be copied once for each partition.
    Local { logical_plan: PlanNodeEnum },
    /// A single-input global operator consuming a partitioned child through
    /// an explicit gather exchange.
    GlobalUnary {
        logical_plan: PlanNodeEnum,
        input: Box<PartitionedPhysicalNode>,
    },
    /// A two-input global operator consuming gathered left and right inputs.
    GlobalBinary {
        logical_plan: PlanNodeEnum,
        left: Box<PartitionedPhysicalNode>,
        right: Box<PartitionedPhysicalNode>,
    },
}

impl PartitionedPhysicalPlan {
    /// Derive a conservative physical decomposition from a logical root and
    /// an already validated layout. Unsupported nodes remain local candidates
    /// and are rejected by the executor builder unless they prove
    /// partition-local; callers can then fall back to a single tree.
    pub fn from_logical(root: PlanNodeEnum, partition_spec: PartitionSpec) -> Self {
        Self {
            partition_spec,
            root: Self::split_node(root),
        }
    }

    pub fn partition_spec(&self) -> &PartitionSpec {
        &self.partition_spec
    }

    pub fn root(&self) -> &PartitionedPhysicalNode {
        &self.root
    }

    fn split_node(node: PlanNodeEnum) -> PartitionedPhysicalNode {
        match node {
            PlanNodeEnum::Sort(ref sort) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(sort.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Limit(ref limit) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(limit.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::TopN(ref top_n) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(top_n.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Dedup(ref dedup) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(dedup.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Aggregate(ref aggregate) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(aggregate.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Window(ref window) => PartitionedPhysicalNode::GlobalUnary {
                input: Box::new(Self::split_node(window.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::InnerJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::LeftJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::RightJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::CrossJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::HashInnerJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::HashLeftJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::FullOuterJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            PlanNodeEnum::SemiJoin(ref join) => Self::global_binary(
                node.clone(),
                join.left_input().clone(),
                join.right_input().clone(),
            ),
            logical_plan => PartitionedPhysicalNode::Local { logical_plan },
        }
    }

    fn global_binary(
        logical_plan: PlanNodeEnum,
        left: PlanNodeEnum,
        right: PlanNodeEnum,
    ) -> PartitionedPhysicalNode {
        PartitionedPhysicalNode::GlobalBinary {
            logical_plan,
            left: Box::new(Self::split_node(left)),
            right: Box::new(Self::split_node(right)),
        }
    }
}

/// Execution plan structure
/// Represents the complete executable plan, including the root node and the plan ID.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// The root node of the planning tree
    pub root: Option<PlanNodeEnum>,

    /// The unique ID of the plan
    pub id: i64,

    /// Optimized time (in microseconds)
    pub optimize_time_in_us: u64,

    /// Of course! Please provide the text you would like to have translated.
    pub format: String,

    /// Execution mode determined by Phase 3 optimizer (Streaming or Materialized)
    pub execution_mode: ExecutionMode,

    /// Reason for execution mode selection (for debugging/logging)
    pub execution_mode_reason: String,

    /// Optional physical partition layout. A layout is consumed only by a
    /// partition-safe streaming plan; unsupported plans fail explicitly.
    pub partition_spec: Option<PartitionSpec>,
}

impl ExecutionPlan {
    /// Create a new execution plan.
    pub fn new(root: Option<PlanNodeEnum>) -> Self {
        Self {
            root,
            id: -1, // This will be allocated later on.
            optimize_time_in_us: 0,
            format: "default".to_string(),
            execution_mode: ExecutionMode::default(),
            execution_mode_reason: "default".to_string(),
            partition_spec: None,
        }
    }

    /// Set the root node of the plan.
    pub fn set_root(&mut self, root: PlanNodeEnum) {
        self.root = Some(root);
    }

    /// Obtain the reference to the root node of the plan.
    pub fn root(&self) -> &Option<PlanNodeEnum> {
        &self.root
    }

    /// Obtain a reference to the variable root node.
    pub fn root_mut(&mut self) -> &mut Option<PlanNodeEnum> {
        &mut self.root
    }

    /// Set the ID for the plan.
    pub fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    /// Set the optimization time
    pub fn set_optimize_time(&mut self, time_us: u64) {
        self.optimize_time_in_us = time_us;
    }

    /// Set the output format
    pub fn set_format(&mut self, format: String) {
        self.format = format;
    }

    /// Set the execution mode (determined by Phase 3 optimizer)
    pub fn set_execution_mode(&mut self, mode: ExecutionMode, reason: &str) {
        self.execution_mode = mode;
        self.execution_mode_reason = reason.to_string();
    }

    /// Get the execution mode
    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    /// Get the execution mode reason
    pub fn execution_mode_reason(&self) -> &str {
        &self.execution_mode_reason
    }

    /// Attach the physical partition layout chosen by the optimizer.
    pub fn set_partition_spec(&mut self, partition_spec: PartitionSpec) {
        self.partition_spec = Some(partition_spec);
    }

    /// Remove partitioning and execute the plan as a single tree.
    pub fn clear_partition_spec(&mut self) {
        self.partition_spec = None;
    }

    pub fn partition_spec(&self) -> Option<&PartitionSpec> {
        self.partition_spec.as_ref()
    }

    /// Calculate the number of nodes in the plan.
    /// Recursively traverse the entire execution plan tree and count all the nodes.
    pub fn node_count(&self) -> usize {
        fn count_nodes(node: &Option<PlanNodeEnum>) -> usize {
            match node {
                Some(n) => {
                    let mut count = 1;
                    for child in n.children() {
                        count += count_nodes(&Some(child.clone()));
                    }
                    count
                }
                None => 0,
            }
        }
        count_nodes(&self.root)
    }
}

/// SubPlan structure
/// Represents a sub-part of the execution plan, which contains the root node and the tail node.
/// Segmented planning for complex queries
#[derive(Debug, Clone)]
pub struct SubPlan {
    /// The root node of the sub-plan
    pub root: Option<PlanNodeEnum>,

    /// The end node of the sub-plan
    /// Used to connect multiple sub-plans
    pub tail: Option<PlanNodeEnum>,
}

impl SubPlan {
    /// Create a new SubPlan.
    pub fn new(root: Option<PlanNodeEnum>, tail: Option<PlanNodeEnum>) -> Self {
        Self { root, tail }
    }

    /// Create a SubPlan that contains only the root node.
    pub fn from_root(root: PlanNodeEnum) -> Self {
        Self {
            root: Some(root.clone()),
            tail: Some(root),
        }
    }

    /// Create a SubPlan that contains only a single node.
    pub fn from_single_node(node: PlanNodeEnum) -> Self {
        Self {
            root: Some(node.clone()),
            tail: Some(node),
        }
    }

    /// Obtain a reference to the root node.
    pub fn root(&self) -> &Option<PlanNodeEnum> {
        &self.root
    }

    /// Obtain the reference to the tail node.
    pub fn tail(&self) -> &Option<PlanNodeEnum> {
        &self.tail
    }

    /// Setting the root node
    pub fn set_root(&mut self, root: PlanNodeEnum) {
        self.root = Some(root);
    }

    /// Setting the tail node
    pub fn set_tail(&mut self, tail: PlanNodeEnum) {
        self.tail = Some(tail);
    }

    /// Check whether SubPlan is empty.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Retrieve all nodes from the SubPlan.
    pub fn collect_nodes(&self) -> Vec<PlanNodeEnum> {
        let mut nodes = Vec::new();

        if let Some(root) = &self.root {
            nodes.push(root.clone());
        }

        if let Some(tail) = &self.tail {
            nodes.push(tail.clone());
        }

        nodes
    }

    /// Merge the two SubPlans
    pub fn merge(&self, other: &SubPlan) -> SubPlan {
        let root = self.root.clone();
        let tail = other.tail.clone();

        SubPlan::new(root, tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_spec_is_optional_and_round_trips() {
        let mut plan = ExecutionPlan::new(None);
        assert!(plan.partition_spec().is_none());

        let spec = PartitionSpec::try_new(vec![0..10, 10..20])
            .expect("non-overlapping ranges should be accepted");
        plan.set_partition_spec(spec.clone());

        let stored = plan.partition_spec().expect("partition spec");
        assert_eq!(stored, &spec);
        assert_eq!(stored.partition_count(), 2);

        plan.clear_partition_spec();
        assert!(plan.partition_spec().is_none());
    }

    #[test]
    fn partition_spec_rejects_empty_and_overlapping_ranges() {
        assert_eq!(
            PartitionSpec::try_new(Vec::new()),
            Err(PartitionSpecError::Empty)
        );
        assert_eq!(
            PartitionSpec::try_new(vec![0..0]),
            Err(PartitionSpecError::EmptyRange { index: 0 })
        );
        assert_eq!(
            PartitionSpec::try_new(vec![0..10, 5..20]),
            Err(PartitionSpecError::UnorderedOrOverlapping { index: 1 })
        );
    }

    #[test]
    fn physical_plan_keeps_global_limit_above_global_sort() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{
            LimitNode, SortNode,
        };

        let start = PlanNodeEnum::Start(StartNode::new());
        let sort = SortNode::new(start, Vec::new()).expect("sort plan should build");
        let limit =
            LimitNode::new(PlanNodeEnum::Sort(sort), 0, 10).expect("limit plan should build");
        let spec = PartitionSpec::try_new(vec![0..10, 10..20]).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Limit(limit), spec);

        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::GlobalUnary { input, .. }
            if matches!(input.as_ref(), PartitionedPhysicalNode::GlobalUnary { .. }))
        );
    }
}
