//! Sorting and Limiting Operations Estimator
//!
//! Provide a cost estimate for the nodes that are subject to sorting restrictions:
//! - Sort
//! - Limit
//! - TopN
//! - Aggregate
//! - Dedup
//! - Sample
//!
//! Based on the actual executor implementation (cf. aggregation.rs, sort.rs, limit.rs):
//! **Aggregate:** The group status is stored using a `HashMap`. The associated costs include the processing of aggregate functions and the performance overhead of hash operations.
//! Sort: Supports Top-N optimization (heap sorting is used when the amount of data exceeds limit * 10).
//! Limit: Simple memory operations; the cost is proportional to the sum of the offset and the limit value.

use super::{get_input_rows, NodeEstimator};
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::PlanNodeEnum;

/// Sorting and Limiting Operations Estimator
pub struct SortLimitEstimator<'a> {
    cost_calculator: &'a CostCalculator,
}

impl<'a> SortLimitEstimator<'a> {
    /// Create a new estimator for sorting constraints.
    pub fn new(cost_calculator: &'a CostCalculator) -> Self {
        Self { cost_calculator }
    }

    /// Estimating the cardinality of the GROUP BY column
    ///
    /// Based on the actual implementation of AggregateExecutor (using a HashMap):
    /// If there is no GROUP BY, return 1 (global aggregation).
    /// Otherwise, the estimation is based on the number of keys and the number of input lines.
    fn estimate_group_by_cardinality(&self, group_keys: &[String], input_rows: u64) -> u64 {
        if group_keys.is_empty() {
            // 全局聚合，只返回一行（如 COUNT(*)）
            return 1;
        }

        // Estimating the cardinality based on the number of GROUP BY keys
        // The more keys there are, the more detailed the grouping will be, and the greater the number of output rows will be.
        // 使用启发式公式：min(input_rows, max(10, input_rows / (2 ^ key_count)))
        let key_count = group_keys.len() as u32;
        let divisor = 2_u64.saturating_pow(key_count).max(1);
        let estimated = (input_rows / divisor).max(10);

        estimated.min(input_rows).max(1)
    }
}

impl<'a> NodeEstimator for SortLimitEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::Sort(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let sort_keys = n.sort_items().len();
                // The Sort node itself does not have a limit, but if there are child Limit nodes, a limit value can be passed through for optimization purposes.
                let cost =
                    self.cost_calculator
                        .calculate_sort_cost(input_rows_val, sort_keys, None);
                // The “Sort” function does not change the number of lines in the text.
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::Limit(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let limit = n.count();
                // Based on the actual implementation of LimitExecutor: The cost is directly proportional to the sum of the offset and the limit value.
                let offset = n.offset();
                let rows_to_process = ((limit.max(0) + offset.max(0)) as u64).min(input_rows_val);
                let cost = self
                    .cost_calculator
                    .calculate_limit_cost(input_rows_val, limit)
                    + rows_to_process as f64 * self.cost_calculator.config().cpu_tuple_cost * 0.1;
                let output_rows = (limit.max(0) as u64).min(input_rows_val);
                Ok((cost, output_rows))
            }
            PlanNodeEnum::TopN(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let limit = n.limit();
                // TopN 使用堆实现，复杂度 O(n log k)
                let cost = self
                    .cost_calculator
                    .calculate_topn_cost(input_rows_val, limit);
                let output_rows = (limit.max(0) as u64).min(input_rows_val);
                Ok((cost, output_rows))
            }
            PlanNodeEnum::Aggregate(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let agg_funcs = n.aggregation_functions().len();
                let group_keys = n.group_keys().len();

                // The calculation cost is based on the actual implementation of the AggregateExecutor.
                // This includes the processing of aggregate functions as well as operations on hash tables.
                let cost = self.cost_calculator.calculate_aggregate_cost(
                    input_rows_val,
                    agg_funcs,
                    group_keys,
                );

                // The number of aggregated output rows is based on the cardinality of the GROUP BY key (the number of keys in the HashMap).
                let output_rows =
                    self.estimate_group_by_cardinality(n.group_keys(), input_rows_val);
                Ok((cost, output_rows))
            }
            PlanNodeEnum::Dedup(_) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let cost = self.cost_calculator.calculate_dedup_cost(input_rows_val);
                // The number of rows has decreased after deduplication (by approximately 70% of the original number of rows).
                let output_rows = (input_rows_val as f64 * 0.7).max(1.0) as u64;
                Ok((cost, output_rows))
            }
            PlanNodeEnum::Sample(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                // SampleNode uses the `count` parameter to specify the number of samples to be taken.
                let sample_count = n.count().max(0) as u64;
                let cost = self.cost_calculator.calculate_sample_cost(input_rows_val);
                // Number of behavior samples collected (not exceeding the number of input rows)
                let output_rows = sample_count.min(input_rows_val);
                Ok((cost, output_rows.max(1)))
            }
            _ => Err(CostError::UnsupportedNodeType(format!(
                "Sort Limit Estimator does not support node type: {:?}",
                std::mem::discriminant(node)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::AggregateFunction;
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;
    use crate::query::planning::plan::core::nodes::operation::sample_node::SampleNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::*;
    use std::sync::Arc;

    fn create_test_calculator() -> CostCalculator {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        CostCalculator::with_config(stats_manager, config)
    }

    #[test]
    fn test_sort_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_desc("age".to_string()),
        ];
        let node = SortNode::new(input, sort_items).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Sort(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_limit_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = LimitNode::new(input, 10, 50).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Limit(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 50);
    }

    #[test]
    fn test_topn_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![SortItem::column_asc("name".to_string())];
        let node = TopNNode::new(input, sort_items, 10).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::TopN(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 10);
    }

    #[test]
    fn test_aggregate_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let group_keys = vec!["category".to_string()];
        let agg_funcs = vec![AggregateFunction::Count(None)];
        let node =
            AggregateNode::new(input, group_keys, agg_funcs).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Aggregate(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
        assert!(output_rows <= 100);
    }

    #[test]
    fn test_dedup_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = DedupNode::new(input).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Dedup(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 70);
    }

    #[test]
    fn test_sample_estimation() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = SampleNode::new(input, 50).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Sample(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 50);
    }

    #[test]
    fn test_unsupported_node_type() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let node = PlanNodeEnum::Start(StartNode::new());
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_limit_with_zero_offset() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = LimitNode::new(input, 0, 100).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Limit(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 1000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_limit_with_large_offset() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = LimitNode::new(input, 500, 100).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Limit(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 1000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_aggregate_with_no_group_by() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let group_keys = vec![];
        let agg_funcs = vec![AggregateFunction::Count(None)];
        let node =
            AggregateNode::new(input, group_keys, agg_funcs).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Aggregate(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_aggregate_with_multiple_group_keys() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let group_keys = vec![
            "category".to_string(),
            "type".to_string(),
            "status".to_string(),
        ];
        let agg_funcs = vec![AggregateFunction::Count(None)];
        let node =
            AggregateNode::new(input, group_keys, agg_funcs).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Aggregate(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 1000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 10);
        assert!(output_rows <= 1000);
    }

    #[test]
    fn test_sample_with_large_count() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = SampleNode::new(input, 1000).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Sample(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_sample_with_zero_count() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = SampleNode::new(input, 0).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Sample(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_sort_with_multiple_sort_keys() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_desc("age".to_string()),
            SortItem::column_asc("score".to_string()),
        ];
        let node = SortNode::new(input, sort_items).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Sort(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_topn_with_large_limit() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![SortItem::column_asc("name".to_string())];
        let node = TopNNode::new(input, sort_items, 1000).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::TopN(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_dedup_with_zero_input() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = DedupNode::new(input).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Dedup(node);

        let child_estimates = vec![NodeCostEstimate::new(0.0, 0.0, 0)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost >= 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_estimate_group_by_cardinality_no_keys() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let group_keys: Vec<String> = vec![];
        let cardinality = estimator.estimate_group_by_cardinality(&group_keys, 100);
        assert_eq!(cardinality, 1);
    }

    #[test]
    fn test_estimate_group_by_cardinality_single_key() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let group_keys = vec!["category".to_string()];
        let cardinality = estimator.estimate_group_by_cardinality(&group_keys, 1000);
        assert!(cardinality >= 10);
        assert!(cardinality <= 1000);
    }

    #[test]
    fn test_estimate_group_by_cardinality_multiple_keys() {
        let calculator = create_test_calculator();
        let estimator = SortLimitEstimator::new(&calculator);

        let group_keys = vec!["category".to_string(), "type".to_string()];
        let cardinality = estimator.estimate_group_by_cardinality(&group_keys, 1000);
        assert!(cardinality >= 10);
        assert!(cardinality <= 1000);
    }
}
