//! Structure definition of the execution plan
//! Contains the ExecutionPlan and SubPlan structures.

use std::ops::Range;
use std::{error::Error, fmt};

use crate::core::types::operators::AggregateFunction;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::logical_plan::LogicalPlan;
use crate::query::planning::plan::PlanNodeEnum;

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

/// Planner-side partition decomposition of a logical plan.
///
/// This is the planner's partition-aware representation, NOT a duplicate
/// execution physical plan.  The arena [`PhysicalPlan`] (in
/// `executor::streaming::plan::PhysicalPlan`) is the sole execution
/// representation — built from this planning decomposition via
/// `PhysicalPlanBuilder::build` and used by the materializer, cache,
/// EXPLAIN, and PROFILE.
///
/// Logical nodes are retained verbatim; this type only describes where
/// they run (local partition trees or one global tree).
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
    /// An operator that can be split into a local partial phase per partition
    /// followed by a global final phase. Currently used for Aggregate.
    /// The factory builds N copies of the partial phase (one per partition),
    /// wraps them with a Gather, and places the final phase on top.
    AggregateSplit {
        logical_plan: PlanNodeEnum,
        input: Box<PartitionedPhysicalNode>,
    },
    /// Dedup operator split into per-partition local dedup followed by
    /// a global dedup after Gather::Concatenate. This reduces data volume
    /// before the Gather exchange and the final global dedup step.
    DistinctSplit {
        logical_plan: PlanNodeEnum,
        input: Box<PartitionedPhysicalNode>,
    },
    /// TopN operator split into per-partition local TopN followed by
    /// a global MergeSort with the same limit. The local TopN phase
    /// keeps only the top N rows per partition, then MergeSort merges
    /// and truncates to the global limit.
    TopNSplit {
        logical_plan: PlanNodeEnum,
        input: Box<PartitionedPhysicalNode>,
    },
    /// Hash repartition exchange for HashInnerJoin and HashLeftJoin.
    /// Both children must be Local trees (no global ops in their subtrees).
    /// The factory builds a HashShuffleJoin that distributes rows by hash
    /// of join keys into `bucket_count` buckets and joins per bucket.
    HashJoinExchange {
        logical_plan: PlanNodeEnum,
        left: Box<PartitionedPhysicalNode>,
        right: Box<PartitionedPhysicalNode>,
        bucket_count: usize,
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
            PlanNodeEnum::TopN(ref top_n) => PartitionedPhysicalNode::TopNSplit {
                input: Box::new(Self::split_node(top_n.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Dedup(ref dedup) => PartitionedPhysicalNode::DistinctSplit {
                input: Box::new(Self::split_node(dedup.input().clone())),
                logical_plan: node,
            },
            PlanNodeEnum::Aggregate(ref aggregate) => {
                if Self::all_functions_support_partial(aggregate.aggregation_functions()) {
                    PartitionedPhysicalNode::AggregateSplit {
                        input: Box::new(Self::split_node(aggregate.input().clone())),
                        logical_plan: node,
                    }
                } else {
                    PartitionedPhysicalNode::GlobalUnary {
                        input: Box::new(Self::split_node(aggregate.input().clone())),
                        logical_plan: node,
                    }
                }
            }
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

    /// Check if all aggregate functions in the node support partial+final
    /// decomposition (COUNT, SUM, MIN, MAX, AVG). Functions like COLLECT,
    /// DISTINCT aggregate, PERCENTILE, etc. must remain global-only.
    fn all_functions_support_partial(funcs: &[AggregateFunction]) -> bool {
        funcs.iter().all(|f| {
            matches!(
                f,
                AggregateFunction::Count(_)
                    | AggregateFunction::Sum(_)
                    | AggregateFunction::Min(_)
                    | AggregateFunction::Max(_)
                    | AggregateFunction::Avg(_)
            )
        })
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

    /// Optional physical partition layout. A layout is consumed only by a
    /// partition-safe streaming plan; unsupported plans fail explicitly.
    pub partition_spec: Option<PartitionSpec>,

    /// Maximum worker threads for intra-query parallelism (P8).
    /// 1 means fully serial. Propagated from PartitioningConfig.
    pub max_workers: usize,

    /// Per-partition output channel capacity for P8 backpressure.
    pub max_buffered_chunks: usize,

    /// The pure logical plan (if conversion succeeded).
    /// Used by cost-based optimization to make physical decisions.
    pub logical_plan: Option<LogicalPlan>,
}

impl ExecutionPlan {
    /// Create a new execution plan.
    pub fn new(root: Option<PlanNodeEnum>) -> Self {
        Self {
            root,
            id: -1,
            optimize_time_in_us: 0,
            format: "default".to_string(),
            partition_spec: None,
            max_workers: 1,
            max_buffered_chunks: 10,
            logical_plan: None,
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

    pub fn set_max_workers(&mut self, max_workers: usize) {
        self.max_workers = max_workers;
    }

    pub fn set_max_buffered_chunks(&mut self, max_buffered_chunks: usize) {
        self.max_buffered_chunks = max_buffered_chunks;
    }

    /// Attach a pure logical plan for cost-based optimization.
    pub fn set_logical_plan(&mut self, logical_plan: LogicalPlan) {
        self.logical_plan = Some(logical_plan);
    }

    /// Access the logical plan, if available.
    pub fn logical_plan(&self) -> Option<&LogicalPlan> {
        self.logical_plan.as_ref()
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

    fn test_source() -> PartitionSource {
        PartitionSource::VertexId {
            tag: "test".to_string(),
        }
    }

    #[test]
    fn partition_spec_is_optional_and_round_trips() {
        let mut plan = ExecutionPlan::new(None);
        assert!(plan.partition_spec().is_none());

        let spec = PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None)
            .expect("non-overlapping ranges should be accepted");
        plan.set_partition_spec(spec.clone());

        let stored = plan.partition_spec().expect("partition spec");
        assert_eq!(stored, &spec);
        assert_eq!(stored.partition_count(), 2);
        assert_eq!(stored.source(), &test_source());

        plan.clear_partition_spec();
        assert!(plan.partition_spec().is_none());
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
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Limit(limit), spec);

        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::GlobalUnary { input, .. }
            if matches!(input.as_ref(), PartitionedPhysicalNode::GlobalUnary { .. }))
        );
    }

    #[test]
    fn aggregate_with_supported_functions_produces_aggregate_split() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let agg = AggregateNode::new(
            start,
            vec!["group".to_string()],
            vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("amount".to_string()),
            ],
        )
        .expect("aggregate plan should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Aggregate(agg), spec);

        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::AggregateSplit { .. }
            ),
            "Expected AggregateSplit for supported functions, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn aggregate_with_unsupported_function_falls_back_to_global_unary() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let agg = AggregateNode::new(
            start,
            vec![],
            vec![AggregateFunction::Collect("x".to_string())],
        )
        .expect("aggregate plan should build");
        let spec = PartitionSpec::try_new(std::iter::once(0..10).collect(), test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Aggregate(agg), spec);

        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::GlobalUnary { .. }),
            "Expected GlobalUnary fallback for unsupported functions, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn dedup_node_produces_distinct_split() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let dedup = DedupNode::new(start).expect("dedup plan should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Dedup(dedup), spec);

        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::DistinctSplit { .. }
            ),
            "Expected DistinctSplit for Dedup, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn topn_node_produces_topn_split() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{SortItem, TopNNode};

        let start = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![SortItem::column_asc("name".to_string())];
        let topn = TopNNode::new(start, sort_items, 10).expect("topn plan should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::TopN(topn), spec);

        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::TopNSplit { .. }),
            "Expected TopNSplit for TopN, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn hash_join_exchange_is_temporarily_disabled_and_falls_back_to_global_binary() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let join = HashInnerJoinNode::new(start.clone(), start, Vec::new(), Vec::new())
            .expect("hash inner join should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical =
            PartitionedPhysicalPlan::from_logical(PlanNodeEnum::HashInnerJoin(join), spec);

        // HashJoinExchange is disabled pending the chunk-boundary fix (R1).
        // See streaming_current_remediation_plan.md §P0.
        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::GlobalBinary { .. }
            ),
            "Expected GlobalBinary fallback (HashJoinExchange is disabled), got {:?}",
            physical.root()
        );
    }

    #[test]
    fn hash_join_exchange_falls_back_to_global_binary_when_children_are_not_local() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::SortNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let sort = SortNode::new(start.clone(), Vec::new()).expect("sort node should build");
        let join = HashInnerJoinNode::new(
            PlanNodeEnum::Sort(sort),
            start.clone(),
            Vec::new(),
            Vec::new(),
        )
        .expect("hash inner join should build");
        let spec = PartitionSpec::try_new(std::iter::once(0..10).collect(), test_source(), None).expect("valid spec");
        let physical =
            PartitionedPhysicalPlan::from_logical(PlanNodeEnum::HashInnerJoin(join), spec);

        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::GlobalBinary { .. }
            ),
            "Expected GlobalBinary fallback when a child has a global node, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn hash_left_join_exchange_is_temporarily_disabled_and_falls_back_to_global_binary() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::join::join_node::HashLeftJoinNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let join = HashLeftJoinNode::new(start.clone(), start, Vec::new(), Vec::new())
            .expect("hash left join should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical =
            PartitionedPhysicalPlan::from_logical(PlanNodeEnum::HashLeftJoin(join), spec);

        // HashJoinExchange is disabled pending the chunk-boundary fix (R1).
        // See streaming_current_remediation_plan.md §P0.
        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::GlobalBinary { .. }
            ),
            "Expected GlobalBinary fallback (HashJoinExchange is disabled), got {:?}",
            physical.root()
        );
    }

    #[test]
    fn hash_join_exchange_is_not_produced_for_non_hash_joins() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let join = InnerJoinNode::new(start.clone(), start, Vec::new(), Vec::new())
            .expect("inner join should build");
        let spec =
            PartitionSpec::try_new(vec![0..10, 10..20], test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::InnerJoin(join), spec);

        assert!(
            matches!(
                physical.root(),
                PartitionedPhysicalNode::GlobalBinary { .. }
            ),
            "Expected GlobalBinary for non-hash join, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn execution_plan_max_workers_defaults() {
        let plan = ExecutionPlan::new(None);
        assert_eq!(plan.max_workers, 1);
        assert_eq!(plan.max_buffered_chunks, 10);
    }

    #[test]
    fn execution_plan_set_max_workers() {
        let mut plan = ExecutionPlan::new(None);
        plan.set_max_workers(4);
        plan.set_max_buffered_chunks(20);
        assert_eq!(plan.max_workers, 4);
        assert_eq!(plan.max_buffered_chunks, 20);
    }

    #[test]
    fn topn_with_limit_zero_uses_topn_split() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{SortItem, TopNNode};

        let start = PlanNodeEnum::Start(StartNode::new());
        let topn = TopNNode::new(start, vec![SortItem::column_asc("x".to_string())], 0)
            .expect("topn plan should build");
        let spec = PartitionSpec::try_new(std::iter::once(0..5).collect(), test_source(), None).expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::TopN(topn), spec);

        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::TopNSplit { .. }),
            "Expected TopNSplit even for limit=0, got {:?}",
            physical.root()
        );
    }
}
