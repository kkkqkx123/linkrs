//! Cycle Detection for Execution Plans
//!
//! This module provides functionality to detect cycles in execution plan graphs.
//! Cycles in execution plans are invalid and can cause infinite loops during execution.
//!
//! # Algorithm
//! Uses depth-first search (DFS) with a visited set and recursion stack to detect
//! back edges that indicate cycles.

use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use std::collections::{HashSet, VecDeque};

/// Result of cycle detection
#[derive(Debug, Clone)]
pub struct CycleDetectionResult {
    /// Whether a cycle was detected
    pub has_cycle: bool,
    /// Node IDs involved in the cycle (if detected)
    pub cycle_path: Vec<i64>,
    /// Error message describing the cycle
    pub error_message: Option<String>,
}

impl CycleDetectionResult {
    /// Create a successful result (no cycle detected)
    pub fn valid() -> Self {
        Self {
            has_cycle: false,
            cycle_path: Vec::new(),
            error_message: None,
        }
    }

    /// Create a failure result (cycle detected)
    pub fn cycle_detected(cycle_path: Vec<i64>) -> Self {
        let error_message = Some(format!(
            "Cycle detected in execution plan: {}",
            cycle_path
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(" -> ")
        ));
        Self {
            has_cycle: true,
            cycle_path,
            error_message,
        }
    }
}

/// Cycle detector for execution plans
pub struct CycleDetector {
    /// Maximum depth to traverse (prevents stack overflow on very deep plans)
    max_depth: usize,
}

impl Default for CycleDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CycleDetector {
    /// Create a new cycle detector with default settings
    pub fn new() -> Self {
        Self { max_depth: 10000 }
    }

    /// Create a cycle detector with a custom maximum depth
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self { max_depth }
    }

    /// Detect cycles in an execution plan starting from the given root node
    ///
    /// # Arguments
    /// * `root` - The root node of the execution plan
    ///
    /// # Returns
    /// A `CycleDetectionResult` indicating whether a cycle was found
    pub fn detect(&self, root: &PlanNodeEnum) -> CycleDetectionResult {
        let mut visited = HashSet::new();
        let mut recursion_stack = HashSet::new();
        let mut path = Vec::new();

        if self.detect_dfs(root, &mut visited, &mut recursion_stack, &mut path, 0) {
            CycleDetectionResult::cycle_detected(path)
        } else {
            CycleDetectionResult::valid()
        }
    }

    /// Depth-first search for cycle detection
    ///
    /// # Arguments
    /// * `node` - Current node being visited
    /// * `visited` - Set of all visited nodes
    /// * `recursion_stack` - Stack of nodes in the current DFS path
    /// * `path` - Output parameter to store the cycle path if found
    /// * `depth` - Current recursion depth
    ///
    /// # Returns
    /// `true` if a cycle is detected, `false` otherwise
    fn detect_dfs(
        &self,
        node: &PlanNodeEnum,
        visited: &mut HashSet<i64>,
        recursion_stack: &mut HashSet<i64>,
        path: &mut Vec<i64>,
        depth: usize,
    ) -> bool {
        if depth > self.max_depth {
            return false;
        }

        let node_id = node.id();

        if recursion_stack.contains(&node_id) {
            path.push(node_id);
            return true;
        }

        if visited.contains(&node_id) {
            return false;
        }

        visited.insert(node_id);
        recursion_stack.insert(node_id);
        path.push(node_id);

        for child in node.children() {
            if self.detect_dfs(child, visited, recursion_stack, path, depth + 1) {
                return true;
            }
        }

        recursion_stack.remove(&node_id);
        path.pop();
        false
    }

    /// Detect cycles using iterative BFS approach (alternative to DFS)
    ///
    /// This method is useful for very deep plans where DFS might cause stack overflow.
    /// Uses Kahn's algorithm for topological sorting.
    ///
    /// # Arguments
    /// * `root` - The root node of the execution plan
    ///
    /// # Returns
    /// A `CycleDetectionResult` indicating whether a cycle was found
    pub fn detect_iterative(&self, root: &PlanNodeEnum) -> CycleDetectionResult {
        let mut in_degree: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        let mut all_nodes: std::collections::HashMap<i64, &PlanNodeEnum> =
            std::collections::HashMap::new();

        self.collect_nodes_with_in_degrees(root, &mut in_degree, &mut all_nodes);

        let mut queue: VecDeque<i64> = VecDeque::new();
        for (&id, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(id);
            }
        }

        let mut processed_count = 0;

        while let Some(node_id) = queue.pop_front() {
            processed_count += 1;

            if let Some(node) = all_nodes.get(&node_id) {
                for child in node.children() {
                    let child_id = child.id();
                    if let Some(degree) = in_degree.get_mut(&child_id) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(child_id);
                        }
                    }
                }
            }
        }

        if processed_count < all_nodes.len() {
            let cycle_nodes: Vec<i64> = all_nodes
                .keys()
                .filter(|id| in_degree.get(id).is_some_and(|&d| d > 0))
                .copied()
                .collect();
            CycleDetectionResult::cycle_detected(cycle_nodes)
        } else {
            CycleDetectionResult::valid()
        }
    }

    /// Collect all nodes and compute their in-degrees
    fn collect_nodes_with_in_degrees<'a>(
        &self,
        node: &'a PlanNodeEnum,
        in_degree: &mut std::collections::HashMap<i64, usize>,
        all_nodes: &mut std::collections::HashMap<i64, &'a PlanNodeEnum>,
    ) {
        let node_id = node.id();

        if all_nodes.contains_key(&node_id) {
            return;
        }

        all_nodes.insert(node_id, node);
        in_degree.entry(node_id).or_insert(0);

        for child in node.children() {
            let child_id = child.id();
            *in_degree.entry(child_id).or_insert(0) += 1;
            self.collect_nodes_with_in_degrees(child, in_degree, all_nodes);
        }
    }

    /// Validate a plan and return a result with detailed error information
    pub fn validate(&self, root: &PlanNodeEnum) -> Result<(), String> {
        let result = self.detect(root);
        if result.has_cycle {
            Err(result
                .error_message
                .unwrap_or_else(|| "Cycle detected".to_string()))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_plan_no_cycle() {
        let detector = CycleDetector::new();
        let start = PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        );
        let result = detector.detect(&start);
        assert!(!result.has_cycle);
    }

    #[test]
    fn test_detect_iterative_no_cycle() {
        let detector = CycleDetector::new();
        let start = PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        );
        let result = detector.detect_iterative(&start);
        assert!(!result.has_cycle);
    }
}
