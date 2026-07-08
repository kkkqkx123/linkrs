//! Aggregation Policy Selector Module
//!
//! Cost-based selection of aggregation strategies: determining the optimal approach between hash aggregation and sort aggregation
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::AggregateStrategySelector;
//! use graphdb::query::optimizer::cost::CostCalculator;
//! use std::sync::Arc;
//!
//! let selector = AggregateStrategySelector::new(cost_calculator);
//! let decision = selector.select_strategy(&context);
//! ```

use std::sync::Arc;

use crate::core::types::ContextualExpression;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::decision::OptimizationDecision;

/// Aggregation policy type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateStrategy {
    /// Hash Aggregation – Storing Group Status Using a HashMap
    /// Applicable to: cases where the number of group keys is high and there is sufficient memory available.
    HashAggregate,
    /// Sorting and Aggregation: First sort the data, then perform the aggregation.
    /// Applicable to: inputs that are already sorted or nearly sorted, and in cases where memory is limited.
    SortAggregate,
    /// Stream Aggregation – An optimized aggregation method when the input data is already sorted.
    /// Applicable to: The input data has already been sorted according to the grouping key.
    StreamingAggregate,
}

impl AggregateStrategy {
    /// Obtain the policy name
    pub fn name(&self) -> &'static str {
        match self {
            AggregateStrategy::HashAggregate => "HashAggregate",
            AggregateStrategy::SortAggregate => "SortAggregate",
            AggregateStrategy::StreamingAggregate => "StreamingAggregate",
        }
    }
}

/// Aggregation policy decision-making
#[derive(Debug, Clone)]
pub struct AggregateStrategyDecision {
    /// Selected Aggregation Strategy
    pub strategy: AggregateStrategy,
    /// Estimated number of output lines
    pub estimated_output_rows: u64,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Estimated memory usage (in bytes)
    pub estimated_memory_bytes: u64,
    /// Reason for the choice
    pub reason: SelectionReason,
}

/// Reasons for the choice of strategy
#[derive(Debug, Clone)]
pub enum SelectionReason {
    /// The input is already sorted, and stream aggregation is being used.
    InputAlreadySorted,
    /// A high cardinality of the grouping key results in better performance for hash aggregation.
    HighCardinality,
    /// The base value of the grouping key is low, which makes sorting and aggregation more efficient.
    LowCardinality,
    /// Memory is limited; choose the sorting algorithm for aggregation.
    MemoryConstrained,
    /// With small amounts of data, hash aggregation is much simpler.
    SmallDataSet,
    /// Large volumes of data: Sorting and aggregation should be performed to avoid memory overflow.
    LargeDataSet,
    /// Cost-based decision-making
    CostBased { hash_cost: f64, sort_cost: f64 },
}

/// Aggregation Policy Selector
///
/// Selecting the optimal aggregation execution strategy based on a cost model
#[derive(Debug)]
pub struct AggregateStrategySelector {
    cost_calculator: Arc<CostCalculator>,
}

/// Contextual information for the selection of aggregation strategies
#[derive(Debug, Clone)]
pub struct AggregateContext {
    /// Number of input lines
    pub input_rows: u64,
    /// List of grouping keys
    pub group_keys: Vec<String>,
    /// Number of aggregate functions
    pub agg_function_count: usize,
    /// Memory limit (in bytes; 0 indicates no limit)
    pub memory_limit: u64,
    /// Are the input data already sorted?
    pub input_is_sorted: bool,
    /// Does the sorting key match the grouping key?
    pub sort_keys_match_group_keys: bool,
    /// Are aggregate expressions deterministic?
    pub is_deterministic: bool,
    /// Aggregation expression complexity score
    pub complexity_score: u32,
    /// Table name (tag name) for statistics lookup
    pub table_name: Option<String>,
}

impl AggregateContext {
    /// Create a new aggregation context.
    pub fn new(input_rows: u64, group_keys: Vec<String>, agg_function_count: usize) -> Self {
        Self {
            input_rows,
            group_keys,
            agg_function_count,
            memory_limit: 0,
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        }
    }

    /// Setting memory limits
    pub fn with_memory_limit(mut self, memory_limit: u64) -> Self {
        self.memory_limit = memory_limit;
        self
    }

    /// Set the input to be sorted.
    pub fn with_sorted_input(mut self, sort_keys_match: bool) -> Self {
        self.input_is_sorted = true;
        self.sort_keys_match_group_keys = sort_keys_match;
        self
    }

    /// Setting expression properties
    pub fn with_expression_analysis(
        mut self,
        is_deterministic: bool,
        complexity_score: u32,
    ) -> Self {
        self.is_deterministic = is_deterministic;
        self.complexity_score = complexity_score;
        self
    }

    /// Set table name for statistics lookup
    pub fn with_table_name(mut self, table_name: Option<String>) -> Self {
        self.table_name = table_name;
        self
    }
}

impl AggregateStrategySelector {
    /// Create a new selector for aggregating policies.
    pub fn new(cost_calculator: Arc<CostCalculator>) -> Self {
        Self { cost_calculator }
    }

    /// Selecting the optimal aggregation strategy
    ///
    /// # Parameters
    /// Context: Information about the context in which the aggregation operation is taking place.
    ///
    /// # Return
    /// Aggregation policy decision-making, including the selected policy and the estimated cost.
    pub fn select_strategy(&self, context: &AggregateContext) -> AggregateStrategyDecision {
        self.select_strategy_internal(context, false)
    }

    /// Selecting the optimal aggregation strategy with memory pressure awareness
    ///
    /// This method considers the global memory pressure threshold from the cost model configuration
    /// to make more robust decisions for large-scale aggregations.
    ///
    /// # Parameters
    /// - `context`: Information about the context in which the aggregation operation is taking place.
    ///
    /// # Return
    /// Aggregation policy decision-making, including the selected policy and the estimated cost.
    pub fn select_strategy_with_memory_pressure(
        &self,
        context: &AggregateContext,
    ) -> AggregateStrategyDecision {
        self.select_strategy_internal(context, true)
    }

    /// Internal method for selecting aggregation strategy
    fn select_strategy_internal(
        &self,
        context: &AggregateContext,
        consider_memory_pressure: bool,
    ) -> AggregateStrategyDecision {
        // If the input is already sorted and the sorting key matches the grouping key, stream aggregation should be used preferentially.
        if context.input_is_sorted && context.sort_keys_match_group_keys {
            return self.create_streaming_aggregate_decision(context);
        }

        // If the expression is non-deterministic, hash aggregation should be used preferentially (to avoid the uncertainties associated with sorting).
        if !context.is_deterministic {
            let group_by_cardinality = self.estimate_group_by_cardinality(context);
            let hash_cost = self.calculate_hash_aggregate_cost(context, group_by_cardinality);
            let hash_memory = self.estimate_hash_memory_usage(context, group_by_cardinality);
            return AggregateStrategyDecision {
                strategy: AggregateStrategy::HashAggregate,
                estimated_output_rows: group_by_cardinality.max(1),
                estimated_cost: hash_cost,
                estimated_memory_bytes: hash_memory,
                reason: SelectionReason::CostBased {
                    hash_cost,
                    sort_cost: hash_cost * 1.5, // Assume that the cost of sorting is higher.
                },
            };
        }

        // Estimating the cardinality of the group key
        let group_by_cardinality = self.estimate_group_by_cardinality(context);

        // Calculate the cost of each strategy.
        let hash_cost = self.calculate_hash_aggregate_cost(context, group_by_cardinality);
        let sort_cost = self.calculate_sort_aggregate_cost(context);

        // Check the memory limitations.
        let hash_memory = self.estimate_hash_memory_usage(context, group_by_cardinality);
        let memory_constrained = context.memory_limit > 0 && hash_memory > context.memory_limit;

        // Check global memory pressure if enabled
        let memory_pressure_threshold =
            self.cost_calculator.config().memory_pressure_threshold as u64;
        let under_memory_pressure =
            consider_memory_pressure && hash_memory > memory_pressure_threshold;

        // Decision-making logic
        let (strategy, reason) = if memory_constrained || under_memory_pressure {
            // Memory is limited or under global memory pressure
            // Priority should be given to sorting and aggregation operations
            if under_memory_pressure && sort_cost < hash_cost * 1.5 {
                // Allow up to 50% performance loss to avoid memory pressure
                (
                    AggregateStrategy::SortAggregate,
                    SelectionReason::MemoryConstrained,
                )
            } else if memory_constrained {
                (
                    AggregateStrategy::SortAggregate,
                    SelectionReason::MemoryConstrained,
                )
            } else {
                // Even under pressure, hash is significantly better
                (
                    AggregateStrategy::HashAggregate,
                    SelectionReason::CostBased {
                        hash_cost,
                        sort_cost,
                    },
                )
            }
        } else if context.input_rows
            < self
                .cost_calculator
                .config()
                .strategy_thresholds
                .small_dataset_threshold
        {
            // With small amounts of data, hash aggregation is simpler and more efficient.
            (
                AggregateStrategy::HashAggregate,
                SelectionReason::SmallDataSet,
            )
        } else if group_by_cardinality
            < self
                .cost_calculator
                .config()
                .strategy_thresholds
                .low_cardinality_threshold
        {
            // With a small base size, sorting and aggregation may be more advantageous (as the data becomes more localized after sorting).
            if sort_cost < hash_cost * 1.2 {
                (
                    AggregateStrategy::SortAggregate,
                    SelectionReason::LowCardinality,
                )
            } else {
                (
                    AggregateStrategy::HashAggregate,
                    SelectionReason::CostBased {
                        hash_cost,
                        sort_cost,
                    },
                )
            }
        } else if group_by_cardinality
            > (context.input_rows as f64
                * self
                    .cost_calculator
                    .config()
                    .strategy_thresholds
                    .high_cardinality_ratio) as u64
        {
            // For high cardinalities (close to unique values), hash aggregation is more advantageous.
            (
                AggregateStrategy::HashAggregate,
                SelectionReason::HighCardinality,
            )
        } else {
            // Based on cost comparison
            if hash_cost <= sort_cost {
                (
                    AggregateStrategy::HashAggregate,
                    SelectionReason::CostBased {
                        hash_cost,
                        sort_cost,
                    },
                )
            } else {
                (
                    AggregateStrategy::SortAggregate,
                    SelectionReason::CostBased {
                        hash_cost,
                        sort_cost,
                    },
                )
            }
        };

        let estimated_cost = match strategy {
            AggregateStrategy::HashAggregate => hash_cost,
            AggregateStrategy::SortAggregate => sort_cost,
            AggregateStrategy::StreamingAggregate => {
                self.calculate_streaming_aggregate_cost(context)
            }
        };

        AggregateStrategyDecision {
            strategy,
            estimated_output_rows: group_by_cardinality.max(1),
            estimated_cost,
            estimated_memory_bytes: match strategy {
                AggregateStrategy::HashAggregate => hash_memory,
                AggregateStrategy::SortAggregate => self.estimate_sort_memory_usage(context),
                AggregateStrategy::StreamingAggregate => {
                    self.estimate_streaming_memory_usage(context)
                }
            },
            reason,
        }
    }

    /// Rapid selection strategy (simplified version, for use in decision-making caching)
    pub fn select_strategy_quick(
        &self,
        input_rows: u64,
        group_key_count: usize,
        _agg_function_count: usize,
    ) -> AggregateStrategy {
        if input_rows < 1000 {
            return AggregateStrategy::HashAggregate;
        }

        // Estimating the cardinality of the grouping key
        let cardinality = self.estimate_cardinality_quick(input_rows, group_key_count);

        if cardinality < 100 {
            AggregateStrategy::SortAggregate
        } else {
            AggregateStrategy::HashAggregate
        }
    }

    /// Estimating the cardinality of the grouping key
    ///
    /// First tries to use property combination statistics if available,
    /// then falls back to heuristic estimation.
    fn estimate_group_by_cardinality(&self, context: &AggregateContext) -> u64 {
        // Try to use property combination statistics for better estimation
        if let Some(table_name) = self.get_table_name_from_context(context) {
            if let Some(cardinality) = self
                .cost_calculator
                .statistics_manager()
                .get_combined_cardinality(Some(&table_name), &context.group_keys)
            {
                return cardinality.min(context.input_rows).max(1);
            }
        }

        // Fall back to heuristic estimation
        self.estimate_cardinality_quick(context.input_rows, context.group_keys.len())
    }

    /// Get table name from aggregate context
    /// This is a helper method to extract table name for statistics lookup
    fn get_table_name_from_context(&self, context: &AggregateContext) -> Option<String> {
        context.table_name.clone()
    }

    /// Quick estimation of the base number
    fn estimate_cardinality_quick(&self, input_rows: u64, key_count: usize) -> u64 {
        if key_count == 0 {
            return 1;
        }

        // Heuristic formula: The base number decreases as the number of keys increases.
        // Assume that for each additional key added, the base value is divided by 2.
        let divisor = 2_u64.saturating_pow(key_count as u32).max(1);
        let estimated = (input_rows / divisor).max(10);

        estimated.min(input_rows).max(1)
    }

    /// Calculating the cost of hash aggregation
    fn calculate_hash_aggregate_cost(
        &self,
        context: &AggregateContext,
        _group_by_cardinality: u64,
    ) -> f64 {
        self.cost_calculator.calculate_aggregate_cost(
            context.input_rows,
            context.agg_function_count,
            context.group_keys.len(),
        )
    }

    /// Calculating the cost of sorting and aggregation operations
    fn calculate_sort_aggregate_cost(&self, context: &AggregateContext) -> f64 {
        // Sorting cost + Aggregation cost
        let sort_cost = self.cost_calculator.calculate_sort_cost(
            context.input_rows,
            context.group_keys.len(),
            None,
        );

        // The aggregated cost after sorting is lower (the data has been grouped).
        let agg_cost = context.input_rows as f64
            * context.agg_function_count as f64
            * self.cost_calculator.config().cpu_operator_cost
            * 0.5; // The cost of aggregation is reduced by half after sorting.

        sort_cost + agg_cost
    }

    /// Calculating the cost of stream aggregation
    fn calculate_streaming_aggregate_cost(&self, context: &AggregateContext) -> f64 {
        // Stream aggregation only requires the cost of aggregation; sorting is not necessary.
        context.input_rows as f64
            * context.agg_function_count as f64
            * self.cost_calculator.config().cpu_operator_cost
    }

    /// Estimating the memory usage of hash aggregation
    fn estimate_hash_memory_usage(
        &self,
        context: &AggregateContext,
        group_by_cardinality: u64,
    ) -> u64 {
        // Estimation of the size of a hash table entry (key + aggregation state)
        let key_size = context.group_keys.len() as u64 * 16; // Assume that each key is 16 bytes in size.
        let agg_state_size = context.agg_function_count as u64 * 24; // Assume that each aggregated state occupies 24 bytes.
        let entry_overhead = 16; // Hash table overhead

        let entry_size = key_size + agg_state_size + entry_overhead;
        group_by_cardinality * entry_size.max(64)
    }

    /// Estimating the memory usage for sorting and aggregation operations
    fn estimate_sort_memory_usage(&self, context: &AggregateContext) -> u64 {
        // Sorting may require caching all the data.
        let row_size = 64; // Assume that each line contains 64 bytes.
        context.input_rows * row_size
    }

    /// Estimating the memory usage of stream aggregation
    fn estimate_streaming_memory_usage(&self, context: &AggregateContext) -> u64 {
        // Stream aggregation only requires maintaining the state of the current group.
        let key_size = context.group_keys.len() as u64 * 16;
        let agg_state_size = context.agg_function_count as u64 * 24;
        (key_size + agg_state_size) * 2 // Double buffering
    }

    /// Creating streaming aggregation decisions
    fn create_streaming_aggregate_decision(
        &self,
        context: &AggregateContext,
    ) -> AggregateStrategyDecision {
        let estimated_cost = self.calculate_streaming_aggregate_cost(context);
        let group_by_cardinality = self.estimate_group_by_cardinality(context);

        AggregateStrategyDecision {
            strategy: AggregateStrategy::StreamingAggregate,
            estimated_output_rows: group_by_cardinality.max(1),
            estimated_cost,
            estimated_memory_bytes: self.estimate_streaming_memory_usage(context),
            reason: SelectionReason::InputAlreadySorted,
        }
    }

    /// Update and optimize the information on the aggregation strategies used in decision-making processes.
    pub fn update_decision(&self, decision: &mut OptimizationDecision, context: &AggregateContext) {
        let strategy_decision = self.select_strategy(context);

        // Encode the aggregation policy information into the `rewrite_rules` used for decision-making.
        // Use a specific rule ID to indicate the selection of the aggregation policy.
        let rule_id = match strategy_decision.strategy {
            AggregateStrategy::HashAggregate => {
                crate::query::optimizer::decision::RewriteRuleId::AggregateOptimization
            }
            AggregateStrategy::SortAggregate => {
                crate::query::optimizer::decision::RewriteRuleId::AggregateOptimization
            }
            AggregateStrategy::StreamingAggregate => {
                crate::query::optimizer::decision::RewriteRuleId::AggregateOptimization
            }
        };

        if !decision.rewrite_rules.contains(&rule_id) {
            decision.rewrite_rules.push(rule_id);
        }
    }

    /// Analyze aggregation expression complexity using expression cost
    ///
    /// Returns the total expression cost for all aggregation functions.
    /// Higher cost indicates more complex expressions that benefit from
    /// HashAggregate (to avoid redundant computations).
    pub fn analyze_aggregation_complexity(&self, agg_functions: &[ContextualExpression]) -> f64 {
        agg_functions
            .iter()
            .filter_map(|f| f.get_expression())
            .map(|expr| self.cost_calculator.calculate_expression_cost(&expr))
            .sum()
    }

    /// Select strategy considering both memory pressure and expression complexity
    ///
    /// This is the most comprehensive strategy selection method that considers:
    /// - Input sorting
    /// - Memory constraints and pressure
    /// - Expression complexity
    /// - Cost comparison
    pub fn select_strategy_comprehensive(
        &self,
        context: &AggregateContext,
        agg_functions: &[ContextualExpression],
    ) -> AggregateStrategyDecision {
        // First check if we can use streaming aggregation
        if context.input_is_sorted && context.sort_keys_match_group_keys {
            return self.create_streaming_aggregate_decision(context);
        }

        // Analyze expression complexity
        let total_expression_cost = self.analyze_aggregation_complexity(agg_functions);
        let avg_expression_cost = if agg_functions.is_empty() {
            0.0
        } else {
            total_expression_cost / agg_functions.len() as f64
        };

        // Get base decision with memory pressure awareness
        let mut decision = self.select_strategy_with_memory_pressure(context);

        // If expressions are complex and we're using SortAggregate,
        // consider switching to HashAggregate to avoid redundant computations
        if decision.strategy == AggregateStrategy::SortAggregate
            && avg_expression_cost > self.cost_calculator.config().function_call_base_cost * 2.0
        {
            let group_by_cardinality = self.estimate_group_by_cardinality(context);
            let hash_cost = self.calculate_hash_aggregate_cost(context, group_by_cardinality);
            let sort_cost = decision.estimated_cost;

            // If hash aggregation is not too much more expensive, use it for complex expressions
            if hash_cost < sort_cost * 1.3 {
                let hash_memory = self.estimate_hash_memory_usage(context, group_by_cardinality);
                decision = AggregateStrategyDecision {
                    strategy: AggregateStrategy::HashAggregate,
                    estimated_output_rows: group_by_cardinality.max(1),
                    estimated_cost: hash_cost,
                    estimated_memory_bytes: hash_memory,
                    reason: SelectionReason::CostBased {
                        hash_cost,
                        sort_cost,
                    },
                };
            }
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::StatisticsManager;

    fn create_test_selector() -> AggregateStrategySelector {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        AggregateStrategySelector::new(cost_calculator)
    }

    #[test]
    fn test_streaming_aggregate_when_sorted() {
        let selector = create_test_selector();
        let context = AggregateContext {
            input_rows: 10000,
            group_keys: vec!["category".to_string()],
            agg_function_count: 2,
            memory_limit: 0,
            input_is_sorted: true,
            sort_keys_match_group_keys: true,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        };

        let decision = selector.select_strategy(&context);
        assert_eq!(decision.strategy, AggregateStrategy::StreamingAggregate);
        matches!(decision.reason, SelectionReason::InputAlreadySorted);
    }

    #[test]
    fn test_hash_aggregate_for_small_data() {
        let selector = create_test_selector();
        let context = AggregateContext {
            input_rows: 500,
            group_keys: vec!["category".to_string()],
            agg_function_count: 1,
            memory_limit: 0,
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        };

        let decision = selector.select_strategy(&context);
        assert_eq!(decision.strategy, AggregateStrategy::HashAggregate);
        matches!(decision.reason, SelectionReason::SmallDataSet);
    }

    #[test]
    fn test_memory_constrained_fallback() {
        let selector = create_test_selector();
        let context = AggregateContext {
            input_rows: 100000,
            group_keys: vec!["category".to_string(), "subcategory".to_string()],
            agg_function_count: 3,
            memory_limit: 1024, // Memory limit of 1 KB (very small)
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        };

        let decision = selector.select_strategy(&context);
        assert_eq!(decision.strategy, AggregateStrategy::SortAggregate);
        matches!(decision.reason, SelectionReason::MemoryConstrained);
    }

    #[test]
    fn test_quick_selection() {
        let selector = create_test_selector();

        // For small amounts of data, hash aggregation should be chosen.
        let strategy = selector.select_strategy_quick(500, 1, 1);
        assert_eq!(strategy, AggregateStrategy::HashAggregate);

        // For large amounts of data with multiple keys (and a low cardinality), sorting and aggregation should be chosen as the appropriate methods for processing the data.
        let strategy = selector.select_strategy_quick(100000, 10, 1);
        assert_eq!(strategy, AggregateStrategy::SortAggregate);

        // For large volumes of data with a high number of unique keys (high cardinality), hash aggregation should be chosen.
        let strategy = selector.select_strategy_quick(100000, 1, 1);
        assert_eq!(strategy, AggregateStrategy::HashAggregate);
    }

    #[test]
    fn test_cardinality_estimation() {
        let selector = create_test_selector();

        // If there is no grouping key, the result should be 1.
        let cardinality = selector.estimate_cardinality_quick(1000, 0);
        assert_eq!(cardinality, 1);

        // The cardinality of a single key should be high.
        let cardinality = selector.estimate_cardinality_quick(10000, 1);
        assert!(cardinality > 100);

        // The cardinality of multiple keys should be relatively low.
        let cardinality = selector.estimate_cardinality_quick(10000, 5);
        assert!(cardinality < 1000);
    }

    #[test]
    fn test_memory_pressure_aware_selection() {
        let selector = create_test_selector();

        // Create a context that would normally choose HashAggregate
        // but with high memory usage that exceeds the threshold
        let context = AggregateContext {
            input_rows: 100000,
            group_keys: vec!["category".to_string()],
            agg_function_count: 2,
            memory_limit: 0, // No explicit limit
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        };

        // Without memory pressure, should likely choose HashAggregate for high cardinality
        let _normal_decision = selector.select_strategy(&context);

        // With memory pressure awareness, check if it considers memory threshold
        let pressure_decision = selector.select_strategy_with_memory_pressure(&context);

        // Both should return valid decisions
        assert!(pressure_decision.estimated_cost > 0.0);
        assert!(pressure_decision.estimated_memory_bytes > 0);
    }

    #[test]
    fn test_analyze_aggregation_complexity() {
        let selector = create_test_selector();

        // Test with empty functions
        let empty_complexity = selector.analyze_aggregation_complexity(&[]);
        assert_eq!(empty_complexity, 0.0);

        // Test with simple expressions
        use crate::core::types::expr::Expression;
        use crate::core::value::Value;
        use crate::query::validator::context::ExpressionAnalysisContext;

        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::Literal(Value::Int(42));
        let id = ctx.register_expression(crate::core::types::expr::ExpressionMeta::new(expr));
        let simple_expr = ContextualExpression::new(id, std::sync::Arc::new(ctx));
        let simple_complexity = selector.analyze_aggregation_complexity(&[simple_expr]);
        assert!(simple_complexity >= 0.0);
    }

    #[test]
    fn test_comprehensive_strategy_selection() {
        let selector = create_test_selector();

        let context = AggregateContext {
            input_rows: 10000,
            group_keys: vec!["category".to_string()],
            agg_function_count: 2,
            memory_limit: 0,
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        };

        // Test comprehensive selection with empty functions
        let decision = selector.select_strategy_comprehensive(&context, &[]);
        assert!(decision.estimated_cost > 0.0);
        assert!(decision.estimated_memory_bytes > 0);
    }
}
