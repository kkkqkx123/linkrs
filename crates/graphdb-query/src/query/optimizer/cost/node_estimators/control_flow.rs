//! Control Flow Node Estimator
//!
//! Provide cost estimates for flow control nodes:
//! - Loop
//! - Select
//! - PassThrough
//! - Argument

use super::{get_input_rows, NodeEstimator};
use crate::query::optimizer::cost::config::CostModelConfig;
use crate::query::optimizer::cost::estimate::NodeCostEstimate;
use crate::query::optimizer::cost::expression_parser::ExpressionParser;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::error::CostError;
use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::{
    LoopNode, SelectNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Control Flow Node Estimator
pub struct ControlFlowEstimator<'a> {
    cost_calculator: &'a CostCalculator,
    config: CostModelConfig,
    expression_parser: ExpressionParser,
}

impl<'a> ControlFlowEstimator<'a> {
    /// Create a new control flow estimator.
    pub fn new(cost_calculator: &'a CostCalculator, config: CostModelConfig) -> Self {
        let expression_parser = ExpressionParser::new(config);
        Self {
            cost_calculator,
            config,
            expression_parser,
        }
    }

    /// Estimating the number of iterations of the Loop node
    fn estimate_loop_iterations(&self, node: &LoopNode) -> u32 {
        let condition = node.condition().to_expression_string();

        // Try to parse the number of iterations using an expression parser.
        if let Some(iterations) = self.expression_parser.parse_loop_iterations(&condition) {
            return iterations;
        }

        // The default configuration values will be used.
        self.config.default_loop_iterations
    }

    /// Estimate the number of branches of the selected node.
    fn estimate_select_branch_count(&self, node: &SelectNode) -> usize {
        let mut count = 0;
        if node.if_branch().is_some() {
            count += 1;
        }
        if node.else_branch().is_some() {
            count += 1;
        }

        if count == 0 {
            self.config.default_select_branches
        } else {
            count
        }
    }
}

impl<'a> NodeEstimator for ControlFlowEstimator<'a> {
    fn estimate(
        &self,
        node: &PlanNodeEnum,
        child_estimates: &[NodeCostEstimate],
    ) -> Result<(f64, u64), CostError> {
        match node {
            PlanNodeEnum::Loop(n) => {
                let body_estimate = child_estimates
                    .first()
                    .copied()
                    .unwrap_or(NodeCostEstimate::new(0.0, 0.0, 0));
                let iterations = self.estimate_loop_iterations(n);
                let cost = self
                    .cost_calculator
                    .calculate_loop_cost(body_estimate.total_cost, iterations);
                // The number of lines output by the `Loop` function is equal to the number of lines output by the loop body multiplied by the number of iterations.
                let output_rows = body_estimate.output_rows.saturating_mul(iterations as u64);
                Ok((cost, output_rows))
            }
            PlanNodeEnum::Select(n) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let branch_count = self.estimate_select_branch_count(n);
                let cost = self
                    .cost_calculator
                    .calculate_select_cost(input_rows_val, branch_count);
                // The number of output lines should be equal to the number of input lines (assuming that on average one branch is selected).
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::PassThrough(_) => {
                let input_rows_val = get_input_rows(child_estimates, 0);
                let cost = self
                    .cost_calculator
                    .calculate_pass_through_cost(input_rows_val);
                Ok((cost, input_rows_val))
            }
            PlanNodeEnum::Argument(_) => Ok((0.0, 1)),
            _ => Err(CostError::UnsupportedNodeType(format!(
                "The control flow estimator does not support the node type: {:?}",
                std::mem::discriminant(node)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::cost::config::CostModelConfig;
    use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::*;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use std::sync::Arc;

    fn create_test_calculator() -> CostCalculator {
        let stats_manager = Arc::new(crate::query::optimizer::stats::StatisticsManager::new());
        let config = CostModelConfig::default();
        CostCalculator::with_config(stats_manager, config)
    }

    fn create_test_expression() -> crate::core::types::ContextualExpression {
        use crate::core::types::expr::ExpressionMeta;
        use crate::core::Expression;
        use crate::query::validator::context::ExpressionAnalysisContext;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = ExpressionMeta::new(Expression::Variable("condition".to_string()));
        let id = ctx.register_expression(expr_meta);
        crate::core::types::ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_loop_estimation() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = LoopNode::new(1, condition);
        node.set_body(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Loop(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_select_estimation() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = SelectNode::new(1, condition);
        node.set_if_branch(PlanNodeEnum::Start(StartNode::new()));
        node.set_else_branch(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Select(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_pass_through_estimation() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let node = PassThroughNode::new(1);
        let plan_node = PlanNodeEnum::PassThrough(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_argument_estimation() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let node = ArgumentNode::new(1, "var_name");
        let plan_node = PlanNodeEnum::Argument(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert_eq!(cost, 0.0);
        assert_eq!(output_rows, 1);
    }

    #[test]
    fn test_unsupported_node_type() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let node = PlanNodeEnum::Start(StartNode::new());
        let child_estimates = vec![];
        let result = estimator.estimate(&node, &child_estimates);

        assert!(result.is_err());
    }

    #[test]
    fn test_select_without_branches() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let node = SelectNode::new(1, condition);
        let plan_node = PlanNodeEnum::Select(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_select_with_only_if_branch() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = SelectNode::new(1, condition);
        node.set_if_branch(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Select(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_select_with_only_else_branch() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = SelectNode::new(1, condition);
        node.set_else_branch(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Select(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 100);
    }

    #[test]
    fn test_loop_with_different_iterations() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = LoopNode::new(1, condition);
        node.set_body(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Loop(node);

        let child_estimates = vec![NodeCostEstimate::new(10.0, 10.0, 100)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert!(output_rows >= 1);
    }

    #[test]
    fn test_pass_through_with_zero_input() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let node = PassThroughNode::new(1);
        let plan_node = PlanNodeEnum::PassThrough(node);

        let child_estimates = vec![NodeCostEstimate::new(0.0, 0.0, 0)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost >= 0.0);
        assert_eq!(output_rows, 0);
    }

    #[test]
    fn test_pass_through_with_large_input() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let node = PassThroughNode::new(1);
        let plan_node = PlanNodeEnum::PassThrough(node);

        let child_estimates = vec![NodeCostEstimate::new(1000.0, 1000.0, 1_000_000)];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost > 0.0);
        assert_eq!(output_rows, 1_000_000);
    }

    #[test]
    fn test_loop_without_child_estimates() {
        let calculator = create_test_calculator();
        let config = CostModelConfig::default();
        let estimator = ControlFlowEstimator::new(&calculator, config);

        let condition = create_test_expression();
        let mut node = LoopNode::new(1, condition);
        node.set_body(PlanNodeEnum::Start(StartNode::new()));
        let plan_node = PlanNodeEnum::Loop(node);

        let child_estimates = vec![];
        let result = estimator.estimate(&plan_node, &child_estimates);

        assert!(result.is_ok());
        let (cost, output_rows) = result.expect("Estimation should succeed");
        assert!(cost >= 0.0);
        assert_eq!(output_rows, 0);
    }
}
