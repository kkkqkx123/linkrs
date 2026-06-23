//! Structure definition of the execution plan
//! Contains the ExecutionPlan and SubPlan structures.

use crate::query::planning::plan::PlanNodeEnum;

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
}

impl ExecutionPlan {
    /// Create a new execution plan.
    pub fn new(root: Option<PlanNodeEnum>) -> Self {
        Self {
            root,
            id: -1, // This will be allocated later on.
            optimize_time_in_us: 0,
            format: "default".to_string(),
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
