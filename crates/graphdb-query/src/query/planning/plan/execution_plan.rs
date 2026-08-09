//! Structure definition of the execution plan
//! Contains the ExecutionPlan and SubPlan structures.

use std::collections::HashMap;

use crate::query::planning::plan::logical_plan::LogicalPlan;
use crate::query::planning::plan::PlanNodeEnum;

// The `PartitionSpec` / `PartitionSource` / `PartitionSpecError` types now
// live in `partition_spec.rs`.  Re-export them here so the public
// `ExecutionPlan` API (which carries an optional `PartitionSpec`) keeps
// compiling without touching the new module path at every call site.
pub use crate::query::planning::plan::partition_spec::{
    PartitionSource, PartitionSpec, PartitionSpecError,
};

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

    /// Why parallel partitioning was not selected (empty = partitioning
    /// active or not requested). Surfaced in EXPLAIN / PROFILE diagnostics.
    pub parallel_fallback_reason: String,

    /// Cost-based decision notes produced during optimization (e.g. subquery
    /// unnesting and join-order rewrites). Surfaced in EXPLAIN diagnostics.
    pub cbo_notes: Vec<String>,

    /// Per-logical-node output row estimates produced by the cost-based
    /// phase, keyed by logical node id. Written into physical operator
    /// specs (`estimated_cardinality`) by the `estimated_rows` metadata pass.
    pub row_estimates: HashMap<i64, u64>,

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
            parallel_fallback_reason: String::new(),
            cbo_notes: Vec::new(),
            row_estimates: HashMap::new(),
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
}
