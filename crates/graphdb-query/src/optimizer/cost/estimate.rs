//! Results of the node cost estimation
//!
//! A data structure for estimating the cost of nodes, which includes:
//! The cost of the node itself (excluding its child nodes)
//! Cumulative cost (including all child nodes)
//! Estimated number of output lines

/// Estimates of node costs and the number of rows
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeCostEstimate {
    /// The cost of the node itself (excluding its child nodes)
    pub node_cost: f64,
    /// Total cost (including all child nodes)
    pub total_cost: f64,
    /// Estimated number of output lines
    pub output_rows: u64,
    /// Estimated memory usage in bytes
    pub memory_usage: usize,
    /// Expression evaluation cost
    pub expression_cost: f64,
}

impl NodeCostEstimate {
    /// Create a new estimate.
    pub fn new(node_cost: f64, total_cost: f64, output_rows: u64) -> Self {
        Self {
            node_cost,
            total_cost,
            output_rows,
            memory_usage: 0,
            expression_cost: 0.0,
        }
    }

    /// Create a new estimate with memory and expression cost.
    pub fn with_memory_and_expression(
        node_cost: f64,
        total_cost: f64,
        output_rows: u64,
        memory_usage: usize,
        expression_cost: f64,
    ) -> Self {
        Self {
            node_cost,
            total_cost,
            output_rows,
            memory_usage,
            expression_cost,
        }
    }

    /// Estimated results for creating leaf nodes (without child nodes)
    pub fn leaf(node_cost: f64, output_rows: u64) -> Self {
        Self {
            node_cost,
            total_cost: node_cost,
            output_rows,
            memory_usage: 0,
            expression_cost: 0.0,
        }
    }

    /// Create a zero-cost estimation result.
    pub fn zero() -> Self {
        Self {
            node_cost: 0.0,
            total_cost: 0.0,
            output_rows: 0,
            memory_usage: 0,
            expression_cost: 0.0,
        }
    }

    /// Combining the estimated results of multiple child nodes
    pub fn combine_children(children: &[Self], node_cost: f64, output_rows: u64) -> Self {
        let child_total_cost: f64 = children.iter().map(|e| e.total_cost).sum();
        let total_memory: usize = children.iter().map(|e| e.memory_usage).sum();
        let total_expression_cost: f64 = children.iter().map(|e| e.expression_cost).sum();
        Self {
            node_cost,
            total_cost: node_cost + child_total_cost,
            output_rows,
            memory_usage: total_memory,
            expression_cost: total_expression_cost,
        }
    }

    /// Obtain the cost ratio (node cost/total cost).
    pub fn cost_ratio(&self) -> f64 {
        if self.total_cost == 0.0 {
            0.0
        } else {
            self.node_cost / self.total_cost
        }
    }

    /// Check whether the estimated results are valid.
    pub fn is_valid(&self) -> bool {
        self.node_cost >= 0.0 && self.total_cost >= 0.0
    }
}

impl Default for NodeCostEstimate {
    fn default() -> Self {
        Self::zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_cost_estimate_new() {
        let estimate = NodeCostEstimate::new(10.0, 100.0, 50);
        assert_eq!(estimate.node_cost, 10.0);
        assert_eq!(estimate.total_cost, 100.0);
        assert_eq!(estimate.output_rows, 50);
        assert_eq!(estimate.memory_usage, 0);
        assert_eq!(estimate.expression_cost, 0.0);
    }

    #[test]
    fn test_node_cost_estimate_with_memory_and_expression() {
        let estimate = NodeCostEstimate::with_memory_and_expression(10.0, 100.0, 50, 1024, 5.0);
        assert_eq!(estimate.node_cost, 10.0);
        assert_eq!(estimate.total_cost, 100.0);
        assert_eq!(estimate.output_rows, 50);
        assert_eq!(estimate.memory_usage, 1024);
        assert_eq!(estimate.expression_cost, 5.0);
    }

    #[test]
    fn test_node_cost_estimate_leaf() {
        let estimate = NodeCostEstimate::leaf(10.0, 50);
        assert_eq!(estimate.node_cost, 10.0);
        assert_eq!(estimate.total_cost, 10.0);
        assert_eq!(estimate.output_rows, 50);
        assert_eq!(estimate.memory_usage, 0);
        assert_eq!(estimate.expression_cost, 0.0);
    }

    #[test]
    fn test_node_cost_estimate_zero() {
        let estimate = NodeCostEstimate::zero();
        assert_eq!(estimate.node_cost, 0.0);
        assert_eq!(estimate.total_cost, 0.0);
        assert_eq!(estimate.output_rows, 0);
        assert_eq!(estimate.memory_usage, 0);
        assert_eq!(estimate.expression_cost, 0.0);
    }

    #[test]
    fn test_combine_children() {
        let child1 = NodeCostEstimate::with_memory_and_expression(10.0, 10.0, 100, 512, 2.0);
        let child2 = NodeCostEstimate::with_memory_and_expression(20.0, 20.0, 200, 1024, 3.0);
        let combined = NodeCostEstimate::combine_children(&[child1, child2], 5.0, 50);

        assert_eq!(combined.node_cost, 5.0);
        assert_eq!(combined.total_cost, 35.0); // 5 + 10 + 20
        assert_eq!(combined.output_rows, 50);
        assert_eq!(combined.memory_usage, 1536); // 512 + 1024
        assert_eq!(combined.expression_cost, 5.0); // 2.0 + 3.0
    }

    #[test]
    fn test_cost_ratio() {
        let estimate = NodeCostEstimate::new(10.0, 100.0, 50);
        assert_eq!(estimate.cost_ratio(), 0.1);

        let zero = NodeCostEstimate::zero();
        assert_eq!(zero.cost_ratio(), 0.0);
    }

    #[test]
    fn test_is_valid() {
        let valid = NodeCostEstimate::new(10.0, 100.0, 50);
        assert!(valid.is_valid());

        let invalid = NodeCostEstimate::new(-1.0, 100.0, 50);
        assert!(!invalid.is_valid());
    }
}
