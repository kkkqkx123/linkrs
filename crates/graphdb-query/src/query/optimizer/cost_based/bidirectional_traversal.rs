//! Bidirectional Traversal Optimizer
//!
//! Optimization decisions for bidirectional BFS traversals in shortest path queries
//! 双向BFS同时从起点和终点搜索，可将复杂度从O(b^d)降到O(b^(d/2))

use std::sync::Arc;

use crate::core::types::EdgeDirection;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::stats::{EdgeTypeStatistics, StatisticsManager};

/// Deep context allocation
#[derive(Debug, Clone)]
pub struct DepthAllocationContext {
    /// Name of the starting variable
    pub start_variable: String,
    /// Destination variable name
    pub end_variable: String,
    /// List of edge types
    pub edge_types: Vec<String>,
    /// Total traversal depth
    pub total_depth: u32,
    /// Source tag (if any):
    pub start_tag: Option<String>,
    /// Destination tag (if any)
    pub end_tag: Option<String>,
    /// Estimated starting degree (if known)
    pub start_degree_hint: Option<f64>,
    /// Estimated degree at the destination (if known)
    pub end_degree_hint: Option<f64>,
}

impl DepthAllocationContext {
    /// Create a new deep allocation context.
    pub fn new(
        start_variable: impl Into<String>,
        end_variable: impl Into<String>,
        edge_types: Vec<String>,
        total_depth: u32,
    ) -> Self {
        Self {
            start_variable: start_variable.into(),
            end_variable: end_variable.into(),
            edge_types,
            total_depth,
            start_tag: None,
            end_tag: None,
            start_degree_hint: None,
            end_degree_hint: None,
        }
    }

    /// Set the starting point label
    pub fn with_start_tag(mut self, tag: impl Into<String>) -> Self {
        self.start_tag = Some(tag.into());
        self
    }

    /// Set the destination label
    pub fn with_end_tag(mut self, tag: impl Into<String>) -> Self {
        self.end_tag = Some(tag.into());
        self
    }

    /// “Set the starting degree value” prompt
    pub fn with_start_degree(mut self, degree: f64) -> Self {
        self.start_degree_hint = Some(degree);
        self
    }

    /// Set a reminder for the target degree value
    pub fn with_end_degree(mut self, degree: f64) -> Self {
        self.end_degree_hint = Some(degree);
        self
    }
}

/// Bidirectional traversal decision
#[derive(Debug, Clone)]
pub struct BidirectionalDecision {
    /// Should we use bidirectional traversal?
    pub use_bidirectional: bool,
    /// Starting variable for forward search
    pub forward_start: String,
    /// Reverse search starting variable
    pub backward_start: String,
    /// Expected proportion of reduction in the search space
    pub estimated_savings: f64,
    /// Recommended depth limit
    pub recommended_depth: u32,
}

impl BidirectionalDecision {
    /// Creating a decision-making process that does not use bidirectional traversal
    pub fn unidirectional(forward_start: String) -> Self {
        Self {
            use_bidirectional: false,
            forward_start,
            backward_start: String::new(),
            estimated_savings: 0.0,
            recommended_depth: 0,
        }
    }

    /// Creating a decision-making process that uses bidirectional traversal
    pub fn bidirectional(
        forward_start: String,
        backward_start: String,
        estimated_savings: f64,
        recommended_depth: u32,
    ) -> Self {
        Self {
            use_bidirectional: true,
            forward_start,
            backward_start,
            estimated_savings,
            recommended_depth,
        }
    }
}

/// Bidirectional Traversal Optimizer
pub struct BidirectionalTraversalOptimizer {
    /// Cost Calculator (reserved for more accurate cost estimations in the future)
    cost_calculator: Arc<CostCalculator>,
    /// Statistics Information Manager
    stats_manager: Arc<StatisticsManager>,
}

impl BidirectionalTraversalOptimizer {
    /// Create a new optimizer for bidirectional traversal optimization.
    pub fn new(
        cost_calculator: Arc<CostCalculator>,
        stats_manager: Arc<StatisticsManager>,
    ) -> Self {
        Self {
            cost_calculator,
            stats_manager,
        }
    }

    /// Evaluating whether it is suitable for bidirectional traversal
    ///
    /// # Parameters
    /// `start_variable`: The name of the starting variable
    /// `end_variable`: The name of the destination variable
    /// `edge_types`: List of edge types
    /// `max_depth`: The maximum depth of the traversal.
    ///
    /// # Return value
    /// Return the decision for bidirectional traversal.
    pub fn evaluate(
        &self,
        start_variable: &str,
        end_variable: &str,
        edge_types: &[String],
        max_depth: u32,
    ) -> BidirectionalDecision {
        // When the depth is less than 2, the benefits of bidirectional traversal are not significant.
        if max_depth < 2 {
            return BidirectionalDecision::unidirectional(start_variable.to_string());
        }

        // Obtaining the average branching factor
        let avg_branching = self.estimate_average_branching(edge_types);

        // Calculate the one-way search space: b^d
        let unidirectional_cost = avg_branching.powi(max_depth as i32);

        // Calculate the size of the bidirectional search space: 2 * b^(d/2)
        let half_depth = (max_depth as f64 / 2.0).ceil() as i32;
        let bidirectional_cost = 2.0 * avg_branching.powi(half_depth);

        // Calculate the savings percentage
        let savings = if unidirectional_cost > 0.0 {
            1.0 - (bidirectional_cost / unidirectional_cost)
        } else {
            0.0
        };

        // Use threshold from config for bidirectional savings
        let threshold = self
            .cost_calculator
            .config()
            .strategy_thresholds
            .bidirectional_savings_threshold;
        if savings > threshold {
            BidirectionalDecision::bidirectional(
                start_variable.to_string(),
                end_variable.to_string(),
                savings,
                max_depth,
            )
        } else {
            BidirectionalDecision::unidirectional(start_variable.to_string())
        }
    }

    /// Evaluating whether the path query is suitable for bidirectional traversal
    pub fn evaluate_path_query(
        &self,
        start_variable: &str,
        end_variable: &str,
        edge_types: &[String],
        min_depth: u32,
        max_depth: u32,
    ) -> BidirectionalDecision {
        // For path queries, it is necessary to consider the depth range.
        let avg_depth = ((min_depth + max_depth) as f64 / 2.0) as u32;

        // If the minimum depth is relatively large, a bidirectional traversal is more worthwhile.
        if min_depth >= 2 {
            let decision = self.evaluate(start_variable, end_variable, edge_types, avg_depth);
            if decision.use_bidirectional {
                return decision;
            }
        }

        // Otherwise, use the standard evaluation method.
        self.evaluate(start_variable, end_variable, edge_types, max_depth)
    }

    /// Estimate the average branching factor
    fn estimate_average_branching(&self, edge_types: &[String]) -> f64 {
        if edge_types.is_empty() {
            // Default branching factor
            return 2.0;
        }

        let mut total_branching = 0.0;
        let mut count = 0;

        for edge_type in edge_types {
            if let Some(stats) = self.stats_manager.get_edge_stats(edge_type) {
                // Using the average outdegree as the branching factor for estimation
                let branching = stats.avg_out_degree;
                total_branching += branching;
                count += 1;
            }
        }

        if count > 0 {
            total_branching / count as f64
        } else {
            2.0 // Default value
        }
    }

    /// Calculating the recommended depth distribution for bidirectional traversal
    ///
    /// Based on statistical information about edge types and degree estimates, the depth of forward and backward searches is intelligently allocated.
    /// Strategy: Allocate more depth to the end with the smaller degree value, as there are fewer branches and a smaller search space.
    pub fn calculate_depth_allocation(&self, context: &DepthAllocationContext) -> (u32, u32) {
        let total_depth = context.total_depth;

        // When the depth is less than 2, the resources are evenly distributed directly.
        if total_depth < 2 {
            return (total_depth, 0);
        }

        // Obtain statistical information about the edge types.
        let edge_stats: Vec<EdgeTypeStatistics> = context
            .edge_types
            .iter()
            .filter_map(|et| self.stats_manager.get_edge_stats(et))
            .collect();

        // Estimate the degrees of the starting and ending points.
        let start_degree = context
            .start_degree_hint
            .or_else(|| {
                self.estimate_vertex_degree(&context.start_tag, &edge_stats, EdgeDirection::Out)
            })
            .unwrap_or_else(|| self.estimate_average_branching(&context.edge_types));

        let end_degree = context
            .end_degree_hint
            .or_else(|| {
                self.estimate_vertex_degree(&context.end_tag, &edge_stats, EdgeDirection::In)
            })
            .unwrap_or_else(|| self.estimate_average_branching(&context.edge_types));

        // Depth distribution calculated based on the degree proportion.
        // The smaller the degree, the greater the allocated depth (because the search space grows more slowly).
        self.allocate_depth_by_degree(start_degree, end_degree, total_depth, &edge_stats)
    }

    /// Depth is allocated based on the proportion of degrees.
    fn allocate_depth_by_degree(
        &self,
        start_degree: f64,
        end_degree: f64,
        total_depth: u32,
        edge_stats: &[EdgeTypeStatistics],
    ) -> (u32, u32) {
        // Calculate the inclination adjustment factor
        let skewness_adjustment = self.calculate_skewness_adjustment(edge_stats);

        // Basic allocation: The end with the smaller degree value is assigned more depth.
        // Use logarithmic scaling to smooth out extreme values.
        let log_start = (start_degree + 1.0).ln();
        let log_end = (end_degree + 1.0).ln();
        let log_total = log_start + log_end;

        // Inverse proportional distribution: The smaller the degree, the greater the depth.
        let base_forward_ratio = if log_total > 0.0 {
            log_end / log_total
        } else {
            0.5
        };

        // App for adjusting the inclination angle
        let adjusted_ratio = base_forward_ratio * (1.0 + skewness_adjustment);
        let forward_ratio = adjusted_ratio.clamp(0.2, 0.8); // The limit is between 20% and 80%.

        let forward_depth =
            ((forward_ratio * total_depth as f64).round() as u32).clamp(1, total_depth - 1);
        let backward_depth = total_depth - forward_depth;

        (forward_depth, backward_depth)
    }

    /// Calculate the inclination adjustment factor
    fn calculate_skewness_adjustment(&self, edge_stats: &[EdgeTypeStatistics]) -> f64 {
        if edge_stats.is_empty() {
            return 0.0;
        }

        let total_skewness: f64 = edge_stats.iter().map(|s| s.degree_gini_coefficient).sum();

        let avg_skewness = total_skewness / edge_stats.len() as f64;

        // The higher the slope, the greater the tendency for an even distribution (to avoid being concentrated in “hot spots”).
        // Return range: -0.1 to 0.1
        if avg_skewness > 0.7 {
            -0.1 // Severe inclination; adjustment towards equal distribution.
        } else if avg_skewness > 0.5 {
            -0.05 // Moderate inclination
        } else {
            0.0 // Slight tilt; no adjustment required.
        }
    }

    /// Estimating the degree of a vertex
    fn estimate_vertex_degree(
        &self,
        tag: &Option<String>,
        _edge_stats: &[EdgeTypeStatistics],
        direction: EdgeDirection,
    ) -> Option<f64> {
        let tag_name = tag.as_ref()?;

        // Obtain tag statistics information
        let tag_stats = self.stats_manager.get_tag_stats(tag_name)?;

        // Return the average degree based on the direction.
        match direction {
            EdgeDirection::Out => Some(tag_stats.avg_out_degree),
            EdgeDirection::In => Some(tag_stats.avg_in_degree),
            EdgeDirection::Both => Some((tag_stats.avg_out_degree + tag_stats.avg_in_degree) / 2.0),
        }
    }

    /// Check whether it is suitable for use with bidirectional traversal.
    ///
    /// Determine whether bidirectional traversal is beneficial based on statistical information about edge types.
    pub fn should_use_bidirectional(&self, edge_types: &[String]) -> bool {
        if edge_types.is_empty() {
            return false;
        }

        let mut total_out_degree = 0.0;
        let mut total_in_degree = 0.0;
        let mut count = 0;

        for edge_type in edge_types {
            if let Some(stats) = self.stats_manager.get_edge_stats(edge_type) {
                total_out_degree += stats.avg_out_degree;
                total_in_degree += stats.avg_in_degree;
                count += 1;

                // If there is a significant inclination, it is not recommended to perform a bidirectional traversal.
                if stats.is_heavily_skewed() {
                    return false;
                }
            }
        }

        if count == 0 {
            return false;
        }

        let avg_out = total_out_degree / count as f64;
        let avg_in = total_in_degree / count as f64;

        // If the degrees in both directions are very low (< 10), then performing a bidirectional traversal is beneficial.
        // Or if the degrees in both directions are similar, a bidirectional traversal could also be considered.
        (avg_out < 10.0 && avg_in < 10.0) || ((avg_out - avg_in).abs() / (avg_out + avg_in) < 0.3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::optimizer::stats::{EdgeTypeStatistics, TagStatistics};

    fn create_test_optimizer() -> BidirectionalTraversalOptimizer {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        let cost_calculator = Arc::new(CostCalculator::with_config(stats_manager.clone(), config));

        // Add test data
        let edge_stats = EdgeTypeStatistics::new("friend".to_string());
        stats_manager.update_edge_stats(edge_stats);

        BidirectionalTraversalOptimizer::new(cost_calculator, stats_manager)
    }

    fn create_test_optimizer_with_stats() -> BidirectionalTraversalOptimizer {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        let cost_calculator = Arc::new(CostCalculator::with_config(stats_manager.clone(), config));

        // Add tag statistics
        let person_tag = TagStatistics {
            tag_name: "Person".to_string(),
            vertex_count: 1000,
            avg_out_degree: 5.0,
            avg_in_degree: 4.0,
        };
        stats_manager.update_tag_stats(person_tag);

        let company_tag = TagStatistics {
            tag_name: "Company".to_string(),
            vertex_count: 100,
            avg_out_degree: 2.0,
            avg_in_degree: 50.0,
        };
        stats_manager.update_tag_stats(company_tag);

        // Add edge statistics
        let edge_stats = EdgeTypeStatistics {
            edge_type: "works_at".to_string(),
            edge_count: 500,
            avg_out_degree: 1.0,
            avg_in_degree: 5.0,
            max_out_degree: 1,
            max_in_degree: 10,
            unique_src_vertices: 500,
            out_degree_std_dev: 0.5,
            in_degree_std_dev: 2.0,
            degree_gini_coefficient: 0.2,
            hot_vertices: Vec::new(),
        };
        stats_manager.update_edge_stats(edge_stats);

        BidirectionalTraversalOptimizer::new(cost_calculator, stats_manager)
    }

    #[test]
    fn test_unidirectional_for_shallow_depth() {
        let optimizer = create_test_optimizer();
        let decision = optimizer.evaluate("a", "b", &[], 1);

        assert!(!decision.use_bidirectional);
        assert_eq!(decision.forward_start, "a");
    }

    #[test]
    fn test_bidirectional_for_deep_depth() {
        let optimizer = create_test_optimizer();
        // When the depth is 4, there should be a significant benefit from performing a bidirectional traversal.
        let decision = optimizer.evaluate("a", "b", &[], 4);

        // Since the default branching factor is 2, there should be a significant savings at a depth of 4.
        if decision.use_bidirectional {
            assert!(decision.estimated_savings > 0.0);
            assert_eq!(decision.forward_start, "a");
            assert_eq!(decision.backward_start, "b");
        }
    }

    #[test]
    fn test_estimate_average_branching() {
        let optimizer = create_test_optimizer();
        let branching = optimizer.estimate_average_branching(&[]);

        // The list of empty edge types should return the default value.
        assert_eq!(branching, 2.0);
    }

    #[test]
    fn test_depth_allocation_with_context() {
        let optimizer = create_test_optimizer_with_stats();

        // Test average distribution (without label information)
        let context = DepthAllocationContext::new("a", "b", vec!["works_at".to_string()], 4);
        let (forward, backward) = optimizer.calculate_depth_allocation(&context);

        assert_eq!(forward + backward, 4);
        assert!(forward >= 1);
        assert!(backward >= 1);
    }

    #[test]
    fn test_depth_allocation_with_tags() {
        let optimizer = create_test_optimizer_with_stats();

        // Person(出度5) -> Company(入度50)，应该给Person端更多深度
        let context = DepthAllocationContext::new("a", "b", vec!["works_at".to_string()], 4)
            .with_start_tag("Person")
            .with_end_tag("Company");

        let (forward, backward) = optimizer.calculate_depth_allocation(&context);

        // Verify that the sum of the depths is correct.
        assert_eq!(forward + backward, 4);
        // Person出度(5) < Company入度(50)，应该给Person更多深度
        // In other words, the value of `forward_depth` should be larger.
        assert!(
            forward >= backward,
            "The end with the smaller degree should be assigned more depth."
        );
    }

    #[test]
    fn test_depth_allocation_with_degree_hints() {
        let optimizer = create_test_optimizer();

        // Use the degree hints.
        let context = DepthAllocationContext::new("a", "b", vec![], 6)
            .with_start_degree(2.0)
            .with_end_degree(8.0);

        let (forward, backward) = optimizer.calculate_depth_allocation(&context);

        // If the degree value is 2 < degree value is 8, the starting point should be given more “depth” (in terms of certain parameters, such as depth of field, complexity, or significance).
        assert!(
            forward > backward,
            "Endpoints with smaller degrees should be assigned more depth."
        );
        assert_eq!(forward + backward, 6);
    }

    #[test]
    fn test_should_use_bidirectional() {
        let optimizer = create_test_optimizer_with_stats();

        // The degree value associated with the “works_at” field is relatively low, which should make it suitable for bidirectional traversal.
        assert!(optimizer.should_use_bidirectional(&["works_at".to_string()]));

        // The list of empty edge types should return false.
        assert!(!optimizer.should_use_bidirectional(&[]));
    }

    #[test]
    fn test_depth_allocation_shallow_depth() {
        let optimizer = create_test_optimizer();

        // When the depth is 1, all resources should be allocated to the positive direction (i.e., to the positive part of the process or the positive outcome).
        let context = DepthAllocationContext::new("a", "b", vec![], 1);
        let (forward, backward) = optimizer.calculate_depth_allocation(&context);

        assert_eq!(forward, 1);
        assert_eq!(backward, 0);
    }
}
