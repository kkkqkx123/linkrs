//! Data processing node estimator
//!
//! Provide cost estimates for data processing nodes:
//! - Filter
//! - Project
//! - Unwind
//! - DataCollect
//! - Start

use super::{get_input_rows, NodeEstimator};
use crate::core::types::BinaryOperator;
use crate::core::Expression;
use crate::query::optimizer::cost::config::CostModelConfig;
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::expression_parser::ExpressionParser;
use crate::query::optimizer::cost::selectivity::SelectivityEstimator;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::core::nodes::graph_operations::UnwindNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Data processing node estimator
pub struct DataProcessingEstimator<'a> {
    cost_calculator: &'a CostCalculator,
    selectivity_estimator: &'a SelectivityEstimator,
    expression_parser: ExpressionParser,
}

impl<'a> DataProcessingEstimator<'a> {
    /// Create a new data processing estimator.
    pub fn new(
        cost_calculator: &'a CostCalculator,
        selectivity_estimator: &'a SelectivityEstimator,
        config: CostModelConfig,
    ) -> Self {
        let expression_parser = ExpressionParser::new(config);
        Self {
            cost_calculator,
            selectivity_estimator,
            expression_parser,
        }
    }

    /// Calculate the number of filter conditions.
    pub fn count_filter_conditions(&self, condition: &Expression) -> usize {
        match condition {
            Expression::Binary { op, left, right } => match op {
                BinaryOperator::And => {
                    self.count_filter_conditions(left) + self.count_filter_conditions(right)
                }
                BinaryOperator::Or => (self.count_filter_conditions(left)
                    + self.count_filter_conditions(right))
                .max(1),
                _ => 1,
            },
            Expression::Unary { .. } => 1,
            Expression::Function { args, .. } => args.iter().map(|_| 1).sum::<usize>().max(1),
            _ => 1,
        }
    }

    /// Estimate the list size of the Unwind nodes
    fn estimate_unwind_list_size(&self, node: &UnwindNode) -> f64 {
        let list_expr = node.list_expression();

        // Try to parse the expression to determine the size of the list.
        let expr_str = list_expr.to_expression_string();
        if let Some(size) = self.expression_parser.parse_list_size(&expr_str) {
            return size;
        }

        // Use the default configuration values.
        self.expression_parser.config().default_unwind_list_size
    }
}

impl<'a> NodeEstimator for DataProcessingEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::Filter(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let condition_expr = match n.condition().expression() {
                    Some(meta) => meta.inner().clone(),
                    None => return Ok((0.0, input_rows_val)),
                };
                let condition_count = self.count_filter_conditions(&condition_expr);
                // Estimate the number of rows after filtering.
                let selectivity = self
                    .selectivity_estimator
                    .estimate_from_expression(&condition_expr, None);
                let output_rows = (input_rows_val as f64 * selectivity).max(1.0) as u64;
                let cost = self
                    .cost_calculator
                    .calculate_filter_cost(input_rows_val, condition_count);
                Ok((cost, output_rows))
            }
            PlanNodeEnum::Project(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let columns = n.columns().len();
                let cost = self
                    .cost_calculator
                    .calculate_project_cost(input_rows_val, columns);
                // The “Project” does not change the number of lines.
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::Unwind(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let list_size = self.estimate_unwind_list_size(n);
                let cost = self
                    .cost_calculator
                    .calculate_unwind_cost(input_rows_val, list_size);
                // “Unwind” will expand each line into a line of the list size.
                let output_rows = (input_rows_val as f64 * list_size) as u64;
                Ok((cost, output_rows.max(1)))
            }
            PlanNodeEnum::DataCollect(_) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let cost = self
                    .cost_calculator
                    .calculate_data_collect_cost(input_rows_val);
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::Start(_) => Ok((0.0, 0)),
            _ => Err(CostError::UnsupportedNodeType(format!(
                "Data Processing Estimator does not support node types: {:?}",
                std::mem::discriminant(node)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::YieldColumn;
    use crate::core::{Expression, Value};
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::*;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    fn create_test_expression() -> crate::core::types::ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = ExpressionMeta::new(Expression::Variable("condition".to_string()));
        let id = ctx.register_expression(expr_meta);
        crate::core::types::ContextualExpression::new(id, ctx)
    }

    fn create_test_calculator_with_selectivity() -> (CostCalculator, SelectivityEstimator) {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        let calculator = CostCalculator::with_config(stats_manager.clone(), config);
        let selectivity_estimator = SelectivityEstimator::new(stats_manager);
        (calculator, selectivity_estimator)
    }

    #[test]
    fn test_filter_estimation() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let condition = create_test_expression();
        let node = FilterNode::new(input, condition).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Filter(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_project_estimation() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = ExpressionMeta::new(Expression::Variable("col".to_string()));
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);
        let columns = vec![YieldColumn {
            expression: ctx_expr,
            alias: "col".to_string(),
            is_matched: false,
        }];
        let node = ProjectNode::new(input, columns).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Project(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_unwind_estimation() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let list_expr = create_test_expression();
        let node = UnwindNode::new(input, "item", list_expr).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Unwind(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_data_collect_estimation() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = DataCollectNode::new(input, "ROW").expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::DataCollect(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_start_estimation() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let node = StartNode::new();
        let plan_node = PlanNodeEnum::Start(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert_eq!(cost, 0.0);
        assert_eq!(output_rows, 0);
    }

    #[test]
    fn test_unsupported_node_type() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let node = PlanNodeEnum::ScanVertices(crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode::new(1, "test_space"));
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_count_filter_conditions_simple() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let condition = Expression::Binary {
            op: BinaryOperator::Equal,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Literal(Value::String("1".to_string()))),
        };
        let count = estimator.count_filter_conditions(&condition);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_filter_conditions_and() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let condition = Expression::Binary {
            op: BinaryOperator::And,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Variable("b".to_string())),
        };
        let count = estimator.count_filter_conditions(&condition);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_filter_conditions_or() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let condition = Expression::Binary {
            op: BinaryOperator::Or,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Variable("b".to_string())),
        };
        let count = estimator.count_filter_conditions(&condition);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_filter_conditions_complex() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let condition = Expression::Binary {
            op: BinaryOperator::And,
            left: Box::new(Expression::Binary {
                op: BinaryOperator::Equal,
                left: Box::new(Expression::Variable("a".to_string())),
                right: Box::new(Expression::Literal(Value::String("1".to_string()))),
            }),
            right: Box::new(Expression::Binary {
                op: BinaryOperator::Or,
                left: Box::new(Expression::Variable("b".to_string())),
                right: Box::new(Expression::Variable("c".to_string())),
            }),
        };
        let count = estimator.count_filter_conditions(&condition);
        assert_eq!(count, 3);
    }

    #[test]
    fn test_filter_with_zero_input() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let condition = create_test_expression();
        let node = FilterNode::new(input, condition).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Filter(node);

        let child_estimates = vec![NodeCostEstimate::new(0.0, 0.0, 0)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost >= 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_project_with_multiple_columns() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let columns = vec![
            YieldColumn {
                expression: {
                    let expr_meta = ExpressionMeta::new(Expression::Variable("a".to_string()));
                    let id = ctx.register_expression(expr_meta);
                    crate::core::types::ContextualExpression::new(id, ctx.clone())
                },
                alias: "a".to_string(),
                is_matched: false,
            },
            YieldColumn {
                expression: {
                    let expr_meta = ExpressionMeta::new(Expression::Variable("b".to_string()));
                    let id = ctx.register_expression(expr_meta);
                    crate::core::types::ContextualExpression::new(id, ctx.clone())
                },
                alias: "b".to_string(),
                is_matched: false,
            },
            YieldColumn {
                expression: {
                    let expr_meta = ExpressionMeta::new(Expression::Variable("c".to_string()));
                    let id = ctx.register_expression(expr_meta);
                    crate::core::types::ContextualExpression::new(id, ctx.clone())
                },
                alias: "c".to_string(),
                is_matched: false,
            },
        ];
        let node = ProjectNode::new(input, columns).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Project(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_unwind_with_large_list() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let list_expr = create_test_expression();
        let node = UnwindNode::new(input, "item", list_expr).expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::Unwind(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 1000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_data_collect_with_large_input() {
        let (calculator, selectivity_estimator) = create_test_calculator_with_selectivity();
        let config = CostModelConfig::default();
        let estimator = DataProcessingEstimator::new(&calculator, &selectivity_estimator, config);

        let input = PlanNodeEnum::Start(StartNode::new());
        let node = DataCollectNode::new(input, "ROW").expect("Node creation should succeed");
        let plan_node = PlanNodeEnum::DataCollect(node);

        let child_estimates = vec![NodeCostEstimate::new(1000.0, 1000.0, 1_000_000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1_000_000);
    }
}
