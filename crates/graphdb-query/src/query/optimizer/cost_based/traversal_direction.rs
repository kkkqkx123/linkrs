//! Image traversal direction optimizer module
//!
//! Select the optimal traversal direction (forward or backward) based on edge statistics.
//!
//! ## Optimization Strategies
//!
//! Choose the direction with a smaller degree value in order to reduce the intermediate results.
//! Consider the impact of super nodes.
//! Support for cost-based direction selection
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::TraversalDirectionOptimizer;
//! use graphdb::query::optimizer::cost::CostCalculator;
//! use std::sync::Arc;
//!
//! let optimizer = TraversalDirectionOptimizer::new(cost_calculator);
//! let decision = optimizer.optimize_direction("KNOWS", None);
//! ```

use std::sync::Arc;

use crate::core::types::EdgeDirection;
use crate::query::optimizer::context::OptimizationContext;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::cost_based::trait_def::OptimizationStrategy;
use crate::query::optimizer::error::OptimizeResult;
use crate::query::optimizer::stats::EdgeTypeStatistics;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

/// Decision on the direction of traversal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraversalDirection {
    /// Forward traversal (outward direction)
    /// Traverse from the source vertex to the target vertex.
    Forward,
    /// Reverse traversal (inward direction)
    /// Traverse from the target vertex to the source vertex.
    Backward,
    /// Bidirectional traversal
    /// Consider both directions at the same time.
    Bidirectional,
}

impl TraversalDirection {
    /// Obtain the name of the direction
    pub fn name(&self) -> &'static str {
        match self {
            TraversalDirection::Forward => "Forward",
            TraversalDirection::Backward => "Backward",
            TraversalDirection::Bidirectional => "Bidirectional",
        }
    }

    /// Convert to EdgeDirection
    pub fn to_edge_direction(&self) -> EdgeDirection {
        match self {
            TraversalDirection::Forward => EdgeDirection::Out,
            TraversalDirection::Backward => EdgeDirection::In,
            TraversalDirection::Bidirectional => EdgeDirection::Both,
        }
    }

    /// Conversion from EdgeDirection
    pub fn from_edge_direction(direction: &EdgeDirection) -> Self {
        match direction {
            EdgeDirection::Out => TraversalDirection::Forward,
            EdgeDirection::In => TraversalDirection::Backward,
            EdgeDirection::Both => TraversalDirection::Bidirectional,
        }
    }
}

/// Reasons for the choice of direction
#[derive(Debug, Clone)]
pub enum DirectionSelectionReason {
    /// Outdegree is less than indegree.
    OutDegreeLower { out_degree: f64, in_degree: f64 },
    /// The in-degree is less than the out-degree.
    InDegreeLower { in_degree: f64, out_degree: f64 },
    /// The degrees are equal or very similar.
    DegreesEqual { out_degree: f64, in_degree: f64 },
    /// Selection based on cost comparison
    CostBased {
        forward_cost: f64,
        backward_cost: f64,
    },
    /// Avoid using super nodes.
    AvoidSuperNode {
        super_node_direction: TraversalDirection,
        threshold: f64,
    },
    /// Statistical information is not available; the default orientation will be used.
    StatsUnavailable,
    /// Explicitly specifying the direction
    ExplicitDirection,
}

/// Decision on the traversal direction
#[derive(Debug, Clone)]
pub struct TraversalDirectionDecision {
    /// The chosen direction
    pub direction: TraversalDirection,
    /// Estimated number of output lines
    pub estimated_output_rows: u64,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Reason for the choice
    pub reason: DirectionSelectionReason,
    /// Average degree (selected direction)
    pub avg_degree: f64,
    /// Does it involve super nodes?
    pub involves_super_node: bool,
}

/// Traversal Direction Optimizer
///
/// Selecting the optimal traversal direction based on edge-based statistical information
#[derive(Debug)]
pub struct TraversalDirectionOptimizer {
    cost_calculator: Arc<CostCalculator>,
    /// Super Node Threshold (A node is considered a super node if its degree exceeds this value.)
    super_node_threshold: f64,
    /// Threshold for degree difference: Differences smaller than this value are considered equal.
    degree_equality_threshold: f64,
}

/// Direction optimization context
#[derive(Debug, Clone)]
pub struct DirectionContext {
    /// Edge type
    pub edge_type: String,
    /// Number of starting nodes
    pub start_nodes: u64,
    /// The explicitly specified direction (if any)
    pub explicit_direction: Option<TraversalDirection>,
    /// Is bidirectional traversal allowed?
    pub allow_bidirectional: bool,
    /// Number of iterations
    pub steps: u32,
}

impl TraversalDirectionOptimizer {
    /// Create a new optimizer for optimizing traversal directions.
    pub fn new(cost_calculator: Arc<CostCalculator>) -> Self {
        // Use thresholds from config if available
        let thresholds = cost_calculator.config().strategy_thresholds;
        Self {
            cost_calculator,
            super_node_threshold: thresholds.traversal_super_node_threshold,
            degree_equality_threshold: thresholds.bidirectional_savings_threshold,
        }
    }

    /// Setting the threshold for super nodes
    pub fn with_super_node_threshold(mut self, threshold: f64) -> Self {
        self.super_node_threshold = threshold;
        self
    }

    /// Set a threshold for equal degrees
    pub fn with_equality_threshold(mut self, threshold: f64) -> Self {
        self.degree_equality_threshold = threshold;
        self
    }

    /// Optimize the direction of the traversal
    ///
    /// # Parameters
    /// “Direction optimization context”
    ///
    /// # Return
    /// Directional decision-making outcome
    pub fn optimize_direction(&self, context: &DirectionContext) -> TraversalDirectionDecision {
        // If a specific direction is explicitly specified, that direction should be given priority.
        if let Some(explicit) = context.explicit_direction {
            return self.create_explicit_decision(context, explicit);
        }

        // Obtain edge statistics information
        let stats = self
            .cost_calculator
            .statistics_manager()
            .get_edge_stats(&context.edge_type);

        match stats {
            Some(edge_stats) => self.optimize_with_stats(context, &edge_stats),
            None => self.create_default_decision(context),
        }
    }

    /// Optimization directions based on statistical information
    fn optimize_with_stats(
        &self,
        context: &DirectionContext,
        stats: &EdgeTypeStatistics,
    ) -> TraversalDirectionDecision {
        let out_degree = stats.avg_out_degree;
        let in_degree = stats.avg_in_degree;

        // Check whether super nodes are involved.
        let forward_is_super = out_degree > self.super_node_threshold;
        let backward_is_super = in_degree > self.super_node_threshold;

        // If both directions involve super nodes, choose the one with the smaller degree (i.e., the one with fewer connections).
        if forward_is_super && backward_is_super {
            let direction = if out_degree <= in_degree {
                TraversalDirection::Forward
            } else {
                TraversalDirection::Backward
            };

            let avg_degree = if out_degree <= in_degree {
                out_degree
            } else {
                in_degree
            };

            return TraversalDirectionDecision {
                direction,
                estimated_output_rows: (context.start_nodes as f64 * avg_degree) as u64,
                estimated_cost: self.calculate_cost(context, true),
                reason: DirectionSelectionReason::AvoidSuperNode {
                    super_node_direction: if out_degree > in_degree {
                        TraversalDirection::Forward
                    } else {
                        TraversalDirection::Backward
                    },
                    threshold: self.super_node_threshold,
                },
                avg_degree,
                involves_super_node: true,
            };
        }

        // If there is only one direction in which the node is a super node, avoid that direction.
        if forward_is_super {
            return TraversalDirectionDecision {
                direction: TraversalDirection::Backward,
                estimated_output_rows: (context.start_nodes as f64 * in_degree) as u64,
                estimated_cost: self.calculate_cost(context, false),
                reason: DirectionSelectionReason::AvoidSuperNode {
                    super_node_direction: TraversalDirection::Forward,
                    threshold: self.super_node_threshold,
                },
                avg_degree: in_degree,
                involves_super_node: true,
            };
        }

        if backward_is_super {
            return TraversalDirectionDecision {
                direction: TraversalDirection::Forward,
                estimated_output_rows: (context.start_nodes as f64 * out_degree) as u64,
                estimated_cost: self.calculate_cost(context, false),
                reason: DirectionSelectionReason::AvoidSuperNode {
                    super_node_direction: TraversalDirection::Backward,
                    threshold: self.super_node_threshold,
                },
                avg_degree: out_degree,
                involves_super_node: true,
            };
        }

        // Choosing the direction based on the comparison of degrees
        let degree_diff = (out_degree - in_degree).abs();
        let degree_ratio = degree_diff / ((out_degree + in_degree) / 2.0);

        if degree_ratio < self.degree_equality_threshold {
            // The degrees are close; the choice is made based on cost calculations.
            self.select_by_cost(context, out_degree, in_degree)
        } else if out_degree < in_degree {
            // The degree of deviation is small; therefore, the positive option should be chosen.
            TraversalDirectionDecision {
                direction: TraversalDirection::Forward,
                estimated_output_rows: (context.start_nodes as f64 * out_degree) as u64,
                estimated_cost: self.calculate_cost(context, false),
                reason: DirectionSelectionReason::OutDegreeLower {
                    out_degree,
                    in_degree,
                },
                avg_degree: out_degree,
                involves_super_node: false,
            }
        } else {
            TraversalDirectionDecision {
                direction: TraversalDirection::Backward,
                estimated_output_rows: (context.start_nodes as f64 * in_degree) as u64,
                estimated_cost: self.calculate_cost(context, false),
                reason: DirectionSelectionReason::InDegreeLower {
                    in_degree,
                    out_degree,
                },
                avg_degree: in_degree,
                involves_super_node: false,
            }
        }
    }

    /// Choosing the direction based on cost
    fn select_by_cost(
        &self,
        context: &DirectionContext,
        out_degree: f64,
        in_degree: f64,
    ) -> TraversalDirectionDecision {
        // Calculate costs with respective degrees
        let forward_cost = self.calculate_cost_with_degree(context, out_degree);
        let backward_cost = self.calculate_cost_with_degree(context, in_degree);

        let (direction, avg_degree) = if forward_cost <= backward_cost {
            (TraversalDirection::Forward, out_degree)
        } else {
            (TraversalDirection::Backward, in_degree)
        };

        let involves_super_node =
            out_degree > self.super_node_threshold || in_degree > self.super_node_threshold;

        TraversalDirectionDecision {
            direction,
            estimated_output_rows: (context.start_nodes as f64 * avg_degree) as u64,
            estimated_cost: forward_cost.min(backward_cost),
            reason: DirectionSelectionReason::CostBased {
                forward_cost,
                backward_cost,
            },
            avg_degree,
            involves_super_node,
        }
    }

    /// Calculating the cost of traversal with a specific degree
    fn calculate_cost_with_degree(&self, context: &DirectionContext, degree: f64) -> f64 {
        let base_cost = self
            .cost_calculator
            .calculate_expand_cost(context.start_nodes, Some(&context.edge_type));

        // Apply penalty for high-degree (super node) scenarios
        let degree_factor = if degree > self.super_node_threshold {
            self.cost_calculator.config().super_node_penalty
        } else {
            // Gradual increase in cost based on degree ratio
            1.0 + (degree / self.super_node_threshold).ln_1p()
        };

        base_cost * degree_factor
    }

    /// Calculating the cost of traversal (legacy method for backward compatibility)
    fn calculate_cost(&self, context: &DirectionContext, is_super: bool) -> f64 {
        let base_cost = self
            .cost_calculator
            .calculate_expand_cost(context.start_nodes, Some(&context.edge_type));

        if is_super {
            base_cost * self.cost_calculator.config().super_node_penalty
        } else {
            base_cost
        }
    }

    /// Making decisions that involve a clear direction
    fn create_explicit_decision(
        &self,
        context: &DirectionContext,
        direction: TraversalDirection,
    ) -> TraversalDirectionDecision {
        // Try to obtain statistical information.
        let avg_degree = self
            .cost_calculator
            .statistics_manager()
            .get_edge_stats(&context.edge_type)
            .map(|s| match direction {
                TraversalDirection::Forward => s.avg_out_degree,
                TraversalDirection::Backward => s.avg_in_degree,
                TraversalDirection::Bidirectional => (s.avg_out_degree + s.avg_in_degree) / 2.0,
            })
            .unwrap_or(2.0);

        let is_super = avg_degree > self.super_node_threshold;

        TraversalDirectionDecision {
            direction,
            estimated_output_rows: (context.start_nodes as f64 * avg_degree) as u64,
            estimated_cost: self.calculate_cost(context, is_super),
            reason: DirectionSelectionReason::ExplicitDirection,
            avg_degree,
            involves_super_node: is_super,
        }
    }

    /// Create a default decision (statistical information not available).
    fn create_default_decision(&self, context: &DirectionContext) -> TraversalDirectionDecision {
        let default_degree = 2.0;

        TraversalDirectionDecision {
            direction: TraversalDirection::Forward, // Default forward direction
            estimated_output_rows: (context.start_nodes as f64 * default_degree) as u64,
            estimated_cost: self.calculate_cost(context, false),
            reason: DirectionSelectionReason::StatsUnavailable,
            avg_degree: default_degree,
            involves_super_node: false,
        }
    }

    /// Quick direction selection (simplified version, for decision-making caching)
    pub fn select_direction_quick(&self, edge_type: &str) -> TraversalDirection {
        let stats = self
            .cost_calculator
            .statistics_manager()
            .get_edge_stats(edge_type);

        match stats {
            Some(s) => {
                if s.avg_out_degree > self.super_node_threshold
                    && s.avg_in_degree <= self.super_node_threshold
                {
                    TraversalDirection::Backward
                } else if s.avg_in_degree > self.super_node_threshold
                    && s.avg_out_degree <= self.super_node_threshold
                    || s.avg_out_degree <= s.avg_in_degree
                {
                    TraversalDirection::Forward
                } else {
                    TraversalDirection::Backward
                }
            }
            None => TraversalDirection::Forward,
        }
    }

    /// Obtaining the degree information of the edges
    pub fn get_degree_info(&self, edge_type: &str) -> Option<DegreeInfo> {
        self.cost_calculator
            .statistics_manager()
            .get_edge_stats(edge_type)
            .map(|s| DegreeInfo {
                avg_out_degree: s.avg_out_degree,
                avg_in_degree: s.avg_in_degree,
                max_out_degree: s.max_out_degree,
                max_in_degree: s.max_in_degree,
                is_out_super: s.avg_out_degree > self.super_node_threshold,
                is_in_super: s.avg_in_degree > self.super_node_threshold,
            })
    }
}

impl OptimizationStrategy for TraversalDirectionOptimizer {
    fn apply(
        &self,
        node: PlanNodeEnum,
        _ctx: &OptimizationContext,
    ) -> OptimizeResult<PlanNodeEnum> {
        // Only optimize ExpandNode
        if let PlanNodeEnum::Expand(mut expand_node) = node {
            // Extract edge type from ExpandNode
            let edge_type = expand_node
                .edge_types()
                .first()
                .cloned()
                .unwrap_or_default();

            // Create direction context
            let direction_context = DirectionContext {
                edge_type,
                start_nodes: 1,            // Default to 1 start node
                explicit_direction: None,  // No explicit direction specified
                allow_bidirectional: true, // Allow bidirectional by default
                steps: 1,                  // Default to 1 step
            };

            // Use underlying optimizer to make decision
            let decision = self.optimize_direction(&direction_context);

            log::debug!(
                "Traversal direction decision: direction={:?}, cost={:.2}, reason={:?}",
                decision.direction,
                decision.estimated_cost,
                decision.reason
            );

            // Update ExpandNode with optimized direction
            expand_node.set_direction(decision.direction.to_edge_direction());
            Ok(PlanNodeEnum::Expand(expand_node))
        } else {
            // Pass through non-ExpandNode
            Ok(node)
        }
    }

    fn name(&self) -> &str {
        "TraversalDirectionOptimizer"
    }

    fn is_enabled(&self) -> bool {
        // Traversal direction strategy is always enabled
        true
    }
}

/// Degree information
#[derive(Debug, Clone)]
pub struct DegreeInfo {
    /// Average attendance
    pub avg_out_degree: f64,
    /// Average Indegree
    pub avg_in_degree: f64,
    /// Maximum Outdegree
    pub max_out_degree: u64,
    /// Maximum In-degree
    pub max_in_degree: u64,
    /// Is the outbound direction a super node?
    pub is_out_super: bool,
    /// Is the inbound direction a super node?
    pub is_in_super: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::StatisticsManager;

    fn create_test_optimizer() -> TraversalDirectionOptimizer {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        TraversalDirectionOptimizer::new(cost_calculator).with_super_node_threshold(100.0)
    }

    #[test]
    fn test_explicit_direction() {
        let optimizer = create_test_optimizer();
        let context = DirectionContext {
            edge_type: "KNOWS".to_string(),
            start_nodes: 100,
            explicit_direction: Some(TraversalDirection::Backward),
            allow_bidirectional: false,
            steps: 1,
        };

        let decision = optimizer.optimize_direction(&context);
        assert_eq!(decision.direction, TraversalDirection::Backward);
        matches!(decision.reason, DirectionSelectionReason::ExplicitDirection);
    }

    #[test]
    fn test_default_direction_when_no_stats() {
        let optimizer = create_test_optimizer();
        let context = DirectionContext {
            edge_type: "UNKNOWN".to_string(),
            start_nodes: 100,
            explicit_direction: None,
            allow_bidirectional: false,
            steps: 1,
        };

        let decision = optimizer.optimize_direction(&context);
        assert_eq!(decision.direction, TraversalDirection::Forward);
        matches!(decision.reason, DirectionSelectionReason::StatsUnavailable);
    }

    #[test]
    fn test_traversal_direction_name() {
        assert_eq!(TraversalDirection::Forward.name(), "Forward");
        assert_eq!(TraversalDirection::Backward.name(), "Backward");
        assert_eq!(TraversalDirection::Bidirectional.name(), "Bidirectional");
    }

    #[test]
    fn test_traversal_direction_conversion() {
        assert_eq!(
            TraversalDirection::from_edge_direction(&EdgeDirection::Out),
            TraversalDirection::Forward
        );
        assert_eq!(
            TraversalDirection::from_edge_direction(&EdgeDirection::In),
            TraversalDirection::Backward
        );
        assert_eq!(
            TraversalDirection::from_edge_direction(&EdgeDirection::Both),
            TraversalDirection::Bidirectional
        );

        assert_eq!(
            TraversalDirection::Forward.to_edge_direction(),
            EdgeDirection::Out
        );
        assert_eq!(
            TraversalDirection::Backward.to_edge_direction(),
            EdgeDirection::In
        );
        assert_eq!(
            TraversalDirection::Bidirectional.to_edge_direction(),
            EdgeDirection::Both
        );
    }

    #[test]
    fn test_quick_selection_no_stats() {
        let optimizer = create_test_optimizer();
        let direction = optimizer.select_direction_quick("UNKNOWN");
        assert_eq!(direction, TraversalDirection::Forward);
    }

    #[test]
    fn test_degree_info() {
        let optimizer = create_test_optimizer();
        // The unknown edge type should return None.
        let info = optimizer.get_degree_info("UNKNOWN");
        assert!(info.is_none());
    }
}
