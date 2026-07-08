//! Connection Order Optimizer Module
//!
//! Cost-based optimization of the join order to select the optimal sequence of joins for multiple tables
//!
//! ## Algorithm Support
//!
//! Dynamic Programming (DP): Provides an accurate solution for determining the optimal order of connections, suitable for a small number of tables (<=8).
//! Greedy algorithm: A fast method for finding an approximate optimal solution, suitable for dealing with a large number of tables.
//! Left-Deep Tree: A classic representation of a connected graph in the form of a tree structure.
//! “Bushy Tree”: A more flexible version of the connected tree structure.
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::JoinOrderOptimizer;
//! use graphdb::query::optimizer::cost::CostCalculator;
//! use std::sync::Arc;
//!
//! let optimizer = JoinOrderOptimizer::new(cost_calculator);
//! let tables = vec![table1, table2, table3];
//! let conditions = vec![join_condition];
//! let decision = optimizer.optimize_join_order(&tables, &conditions);
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::types::ContextualExpression;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::decision::{JoinAlgorithm, JoinOrderDecision};

/// Table information
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Table identifier (variable name)
    pub id: String,
    /// Estimated number of lines
    pub estimated_rows: u64,
    /// Selective (0.0 ~ 1.0)
    pub selectivity: f64,
    /// Is there an index available?
    pub has_index: bool,
    /// The unique identifier of the table (used for bit operations)
    pub bit_id: u32,
}

impl TableInfo {
    /// Create information for a new table.
    pub fn new(id: String, estimated_rows: u64) -> Self {
        Self {
            id,
            estimated_rows,
            selectivity: 1.0,
            has_index: false,
            bit_id: 0,
        }
    }

    /// Set the option to be selective.
    pub fn with_selectivity(mut self, selectivity: f64) -> Self {
        self.selectivity = selectivity.clamp(0.0, 1.0);
        self
    }

    /// Set whether an index is to be created.
    pub fn with_index(mut self, has_index: bool) -> Self {
        self.has_index = has_index;
        self
    }

    /// Set Bit ID
    pub fn with_bit_id(mut self, bit_id: u32) -> Self {
        self.bit_id = bit_id;
        self
    }
}

/// Connection conditions
#[derive(Debug, Clone)]
pub struct JoinCondition {
    /// Left table ID
    pub left_table: String,
    /// Right table ID
    pub right_table: String,
    /// Connection selectivity (estimated proportion of successful connection outcomes)
    pub selectivity: f64,
    /// Connection expression
    pub expression: Option<ContextualExpression>,
}

impl JoinCondition {
    /// Create new connection conditions.
    pub fn new(left_table: String, right_table: String) -> Self {
        Self {
            left_table,
            right_table,
            selectivity: 0.3, // Default selection ratio: 30%
            expression: None,
        }
    }

    /// Set the option to be selective.
    pub fn with_selectivity(mut self, selectivity: f64) -> Self {
        self.selectivity = selectivity.clamp(0.0, 1.0);
        self
    }

    /// Setting the connection expression
    pub fn with_expression(mut self, expression: ContextualExpression) -> Self {
        self.expression = Some(expression);
        self
    }
}

/// Connection Order Optimizer
#[derive(Debug)]
pub struct JoinOrderOptimizer {
    cost_calculator: Arc<CostCalculator>,
    /// Threshold for the size of the dynamic programming table (if this value is exceeded, the greedy algorithm is used)
    dp_threshold: usize,
}

/// Subproblem solution (used in dynamic programming)
#[derive(Debug, Clone)]
struct SubproblemSolution {
    /// The set of included tables (bitmask)
    pub table_set: u32,
    /// The last table that was connected
    pub last_table: String,
    /// Total cost
    pub total_cost: f64,
    /// Of course! Please provide the text you would like to have translated.
    pub output_rows: u64,
    /// Connected tree (represented as a string)
    pub join_tree: String,
}

/// Results of the optimization of the connection sequence
#[derive(Debug, Clone)]
pub struct JoinOrderResult {
    /// Optimal connection order (list of table IDs)
    pub order: Vec<String>,
    /// The choice of algorithm for each connection
    pub algorithms: Vec<JoinAlgorithm>,
    /// Total estimated cost
    pub total_cost: f64,
    /// Final estimated number of output lines
    pub final_output_rows: u64,
    /// The optimization algorithms used
    pub optimization_method: OptimizationMethod,
}

/// Optimization methods
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationMethod {
    /// Dynamic Programming
    DynamicProgramming,
    /// Greedy algorithm
    Greedy,
    /// Heuristic approach (too few examples available)
    Heuristic,
}

impl JoinOrderOptimizer {
    /// Create a new connection order optimizer.
    pub fn new(cost_calculator: Arc<CostCalculator>) -> Self {
        Self {
            cost_calculator,
            dp_threshold: 8, // By default, dynamic programming is used for datasets with fewer than 8 tables.
        }
    }

    /// Setting the DP threshold
    pub fn with_dp_threshold(mut self, threshold: usize) -> Self {
        self.dp_threshold = threshold;
        self
    }

    /// Optimize the order of the connections.
    ///
    /// # Parameters
    /// “tables”: The list of tables involved in the connection.
    /// “conditions”: A list of connection conditions.
    ///
    /// # Return
    /// Results of the optimization of the connection sequence
    pub fn optimize_join_order(
        &self,
        tables: &[TableInfo],
        conditions: &[JoinCondition],
    ) -> JoinOrderResult {
        if tables.len() <= 1 {
            return JoinOrderResult {
                order: tables.iter().map(|t| t.id.clone()).collect(),
                algorithms: Vec::new(),
                total_cost: 0.0,
                final_output_rows: tables.first().map(|t| t.estimated_rows).unwrap_or(0),
                optimization_method: OptimizationMethod::Heuristic,
            };
        }

        if tables.len() <= self.dp_threshold {
            self.optimize_with_dp(tables, conditions)
        } else {
            self.optimize_with_greedy(tables, conditions)
        }
    }

    /// Optimize the connection order using dynamic programming.
    fn optimize_with_dp(
        &self,
        tables: &[TableInfo],
        conditions: &[JoinCondition],
    ) -> JoinOrderResult {
        let n = tables.len();

        // Construct a lookup table for connection conditions
        let condition_map = self.build_condition_map(conditions);

        // DP table: key = bit mask representing the set of tables; value = the optimal solution
        let mut dp: HashMap<u32, SubproblemSolution> = HashMap::new();

        // Initialization: The case of a single table
        for table in tables {
            let solution = SubproblemSolution {
                table_set: 1 << table.bit_id,
                last_table: table.id.clone(),
                total_cost: 0.0,
                output_rows: table.estimated_rows,
                join_tree: table.id.clone(),
            };
            dp.insert(solution.table_set, solution);
        }

        // Dynamic Programming: Constructing subsets from smallest to largest
        for subset_size in 2..=n {
            for subset in self.generate_subsets(n, subset_size) {
                let mut best_solution: Option<SubproblemSolution> = None;

                // Try to decompose the subset into two non-empty subsets.
                for table in tables {
                    let table_bit = 1 << table.bit_id;
                    if subset & table_bit == 0 {
                        continue;
                    }

                    let remaining = subset ^ table_bit;
                    if remaining == 0 {
                        continue;
                    }

                    // Find the optimal solution for the remaining part.
                    if let Some(left_solution) = dp.get(&remaining) {
                        // Calculating the connection cost
                        let (join_cost, output_rows) = self.calculate_join_cost(
                            left_solution.output_rows,
                            table.estimated_rows,
                            &table.id,
                            &condition_map,
                        );

                        let total_cost = left_solution.total_cost + join_cost;

                        let solution = SubproblemSolution {
                            table_set: subset,
                            last_table: table.id.clone(),
                            total_cost,
                            output_rows,
                            join_tree: format!("Join({}, {})", left_solution.join_tree, table.id),
                        };

                        if best_solution
                            .as_ref()
                            .is_none_or(|best| solution.total_cost < best.total_cost)
                        {
                            best_solution = Some(solution);
                        }
                    }
                }

                if let Some(solution) = best_solution {
                    dp.insert(subset, solution);
                }
            }
        }

        // Obtaining the optimal solution
        let full_set = (1 << n) - 1;
        let best_solution = dp
            .get(&full_set)
            .cloned()
            .unwrap_or_else(|| SubproblemSolution {
                table_set: full_set,
                last_table: tables
                    .last()
                    .expect("tables collection is not empty")
                    .id
                    .clone(),
                total_cost: f64::MAX,
                output_rows: 0,
                join_tree: "fallback".to_string(),
            });

        // Reorganize the order of the connections.
        let order = self.reconstruct_order(&best_solution, &dp, tables);
        let algorithms = self.select_algorithms(&order, conditions, tables);

        JoinOrderResult {
            order,
            algorithms,
            total_cost: best_solution.total_cost,
            final_output_rows: best_solution.output_rows,
            optimization_method: OptimizationMethod::DynamicProgramming,
        }
    }

    /// Optimize the connection order using a greedy algorithm.
    fn optimize_with_greedy(
        &self,
        tables: &[TableInfo],
        conditions: &[JoinCondition],
    ) -> JoinOrderResult {
        let condition_map = self.build_condition_map(conditions);
        let mut remaining: HashSet<String> = tables.iter().map(|t| t.id.clone()).collect();
        let table_map: HashMap<String, &TableInfo> =
            tables.iter().map(|t| (t.id.clone(), t)).collect();

        let mut order = Vec::new();
        let mut algorithms = Vec::new();
        let mut total_cost = 0.0;
        let mut current_rows = 0u64;

        // Select the starting table (the one with the fewest rows).
        if let Some(start_table) = tables.iter().min_by_key(|t| t.estimated_rows) {
            order.push(start_table.id.clone());
            remaining.remove(&start_table.id);
            current_rows = start_table.estimated_rows;
        }

        // Greedyly choosing the next table…
        while !remaining.is_empty() {
            let mut best_next: Option<(String, f64, u64)> = None;

            for table_id in &remaining {
                if let Some(table) = table_map.get(table_id) {
                    let (cost, output_rows) = self.calculate_join_cost(
                        current_rows,
                        table.estimated_rows,
                        &table.id,
                        &condition_map,
                    );

                    if best_next
                        .as_ref()
                        .is_none_or(|(_, best_cost, _)| cost < *best_cost)
                    {
                        best_next = Some((table_id.clone(), cost, output_rows));
                    }
                }
            }

            if let Some((next_id, cost, output_rows)) = best_next {
                // Retrieve the index information for the current table and the next table.
                let current_has_index = order
                    .last()
                    .and_then(|id| table_map.get(id))
                    .map(|t| t.has_index)
                    .unwrap_or(false);
                let next_has_index = table_map
                    .get(&next_id)
                    .map(|t| t.has_index)
                    .unwrap_or(false);
                let current_id = order.last().cloned().unwrap_or_default();

                order.push(next_id.clone());
                remaining.remove(&next_id);
                total_cost += cost;

                // Choosing a connection algorithm
                let algorithm = self.select_algorithm(
                    current_rows,
                    output_rows,
                    current_has_index,
                    next_has_index,
                    &current_id,
                    &next_id,
                );
                algorithms.push(algorithm);

                current_rows = output_rows;
            } else {
                break;
            }
        }

        JoinOrderResult {
            order,
            algorithms,
            total_cost,
            final_output_rows: current_rows,
            optimization_method: OptimizationMethod::Greedy,
        }
    }

    /// Construct a lookup table for connection conditions
    fn build_condition_map(&self, conditions: &[JoinCondition]) -> HashMap<(String, String), f64> {
        let mut map = HashMap::new();
        for cond in conditions {
            let key = (cond.left_table.clone(), cond.right_table.clone());
            let reverse_key = (cond.right_table.clone(), cond.left_table.clone());
            map.insert(key, cond.selectivity);
            map.insert(reverse_key, cond.selectivity);
        }
        map
    }

    /// Generate a subset of the specified size.
    fn generate_subsets(&self, n: usize, k: usize) -> Vec<u32> {
        let mut result = Vec::new();
        self.generate_subsets_recursive(0, n, k, 0, &mut result);
        result
    }

    fn generate_subsets_recursive(
        &self,
        start: usize,
        n: usize,
        k: usize,
        current: u32,
        result: &mut Vec<u32>,
    ) {
        if k == 0 {
            result.push(current);
            return;
        }
        if start >= n {
            return;
        }
        for i in start..n {
            self.generate_subsets_recursive(i + 1, n, k - 1, current | (1 << i), result);
        }
    }

    /// Calculating the connection cost
    fn calculate_join_cost(
        &self,
        left_rows: u64,
        right_rows: u64,
        right_table: &str,
        condition_map: &HashMap<(String, String), f64>,
    ) -> (f64, u64) {
        // Finding the connection for selectivity
        let selectivity = condition_map
            .iter()
            .find(|((l, _), _)| l == right_table)
            .map(|(_, s)| *s)
            .unwrap_or(0.3);

        // Count the number of output lines.
        let output_rows = ((left_rows as f64 * right_rows as f64 * selectivity) as u64).max(1);

        // Calculate the connection cost (using a hash join).
        let cost = self
            .cost_calculator
            .calculate_hash_join_cost(left_rows, right_rows);

        (cost, output_rows)
    }

    /// Choosing a connection algorithm
    ///
    /// Selecting the optimal connection algorithm based on a cost model:
    /// 1. If an index is available on one side and the amount of data is moderate, prefer an index-based join.
    /// 2. Compare the costs of hash join and nested loop join, and choose the one with the lower cost.
    /// 3. When using hash joins, it is advisable to choose the smaller table as the building side.
    fn select_algorithm(
        &self,
        left_rows: u64,
        right_rows: u64,
        left_has_index: bool,
        right_has_index: bool,
        left_id: &str,
        right_id: &str,
    ) -> JoinAlgorithm {
        // Threshold definition
        const NESTED_LOOP_MAX_ROWS: u64 = 100; // The maximum number of rows that can be connected using nested loops
        const INDEX_JOIN_MAX_ROWS: u64 = 10000; // The maximum number of rows applicable for index joins

        // Strategy 1: If an index is available on one side and the amount of data on the other side is moderate, use the index to perform the join operation.
        if left_has_index && right_rows <= INDEX_JOIN_MAX_ROWS {
            return JoinAlgorithm::IndexJoin {
                indexed_side: left_id.to_string(),
            };
        }
        if right_has_index && left_rows <= INDEX_JOIN_MAX_ROWS {
            return JoinAlgorithm::IndexJoin {
                indexed_side: right_id.to_string(),
            };
        }

        // Strategy 2: If the amount of data is small in each case, use nested loops for joining the data (to avoid the overhead associated with building hash tables).
        if left_rows <= NESTED_LOOP_MAX_ROWS && right_rows <= NESTED_LOOP_MAX_ROWS {
            return JoinAlgorithm::NestedLoopJoin {
                outer: left_id.to_string(),
                inner: right_id.to_string(),
            };
        }

        // Strategy 3: Use hash joins by default, and select smaller tables for the construction side.
        if left_rows <= right_rows {
            JoinAlgorithm::HashJoin {
                build_side: left_id.to_string(),
                probe_side: right_id.to_string(),
            }
        } else {
            JoinAlgorithm::HashJoin {
                build_side: right_id.to_string(),
                probe_side: left_id.to_string(),
            }
        }
    }

    /// Reorganize the order of the connections.
    fn reconstruct_order(
        &self,
        solution: &SubproblemSolution,
        dp: &HashMap<u32, SubproblemSolution>,
        tables: &[TableInfo],
    ) -> Vec<String> {
        let mut order = Vec::new();
        let mut current_set = solution.table_set;

        // Reconstruct from the back to the front
        while current_set != 0 {
            if let Some(sol) = dp.get(&current_set) {
                order.push(sol.last_table.clone());

                // Find the corresponding table and clear the bit.
                if let Some(table) = tables.iter().find(|t| t.id == sol.last_table) {
                    current_set &= !(1 << table.bit_id);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        order.reverse();
        order
    }

    /// Select an algorithm for the determination of the connection order.
    fn select_algorithms(
        &self,
        order: &[String],
        _conditions: &[JoinCondition],
        tables: &[TableInfo],
    ) -> Vec<JoinAlgorithm> {
        let mut algorithms = Vec::new();
        let table_map: HashMap<String, &TableInfo> =
            tables.iter().map(|t| (t.id.clone(), t)).collect();

        for i in 1..order.len() {
            let left = &order[i - 1];
            let right = &order[i];

            // Obtain index information for the table
            let left_info = table_map.get(left);
            let right_info = table_map.get(right);

            let left_rows = left_info.map(|t| t.estimated_rows).unwrap_or(0);
            let right_rows = right_info.map(|t| t.estimated_rows).unwrap_or(0);
            let left_has_index = left_info.map(|t| t.has_index).unwrap_or(false);
            let right_has_index = right_info.map(|t| t.has_index).unwrap_or(false);

            // Select using a cost-based algorithm
            let algorithm = self.select_algorithm(
                left_rows,
                right_rows,
                left_has_index,
                right_has_index,
                left,
                right,
            );

            algorithms.push(algorithm);
        }

        algorithms
    }

    /// Generate a `JoinOrderDecision`
    pub fn to_decision(&self, result: &JoinOrderResult) -> JoinOrderDecision {
        let mut decision = JoinOrderDecision::empty();

        for (i, table) in result.order.iter().enumerate() {
            if i < result.algorithms.len() {
                decision.add_join_step(table.clone(), result.algorithms[i].clone());
            } else {
                decision.add_join_step(
                    table.clone(),
                    JoinAlgorithm::HashJoin {
                        build_side: "default".to_string(),
                        probe_side: "default".to_string(),
                    },
                );
            }
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::StatisticsManager;

    fn create_test_optimizer() -> JoinOrderOptimizer {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        JoinOrderOptimizer::new(cost_calculator)
    }

    fn create_test_tables() -> Vec<TableInfo> {
        vec![
            TableInfo::new("A".to_string(), 1000).with_bit_id(0),
            TableInfo::new("B".to_string(), 500).with_bit_id(1),
            TableInfo::new("C".to_string(), 2000).with_bit_id(2),
        ]
    }

    #[test]
    fn test_single_table() {
        let optimizer = create_test_optimizer();
        let tables = vec![TableInfo::new("A".to_string(), 1000)];
        let result = optimizer.optimize_join_order(&tables, &[]);

        assert_eq!(result.order.len(), 1);
        assert_eq!(result.order[0], "A");
        assert_eq!(result.total_cost, 0.0);
    }

    #[test]
    fn test_two_tables() {
        let optimizer = create_test_optimizer();
        let tables = vec![
            TableInfo::new("A".to_string(), 1000).with_bit_id(0),
            TableInfo::new("B".to_string(), 500).with_bit_id(1),
        ];
        let conditions = vec![JoinCondition::new("A".to_string(), "B".to_string())];

        let result = optimizer.optimize_join_order(&tables, &conditions);

        assert_eq!(result.order.len(), 2);
        assert!(!result.algorithms.is_empty());
    }

    #[test]
    fn test_dp_vs_greedy() {
        let optimizer = create_test_optimizer();
        let tables = create_test_tables();
        let conditions = vec![
            JoinCondition::new("A".to_string(), "B".to_string()).with_selectivity(0.1),
            JoinCondition::new("B".to_string(), "C".to_string()).with_selectivity(0.2),
        ];

        // Use DP (number of tables <= 8).
        let result = optimizer.optimize_join_order(&tables, &conditions);
        assert_eq!(
            result.optimization_method,
            OptimizationMethod::DynamicProgramming
        );
        assert_eq!(result.order.len(), 3);
    }

    #[test]
    fn test_table_with_selectivity() {
        let table = TableInfo::new("A".to_string(), 1000)
            .with_selectivity(0.5)
            .with_index(true);

        assert_eq!(table.selectivity, 0.5);
        assert!(table.has_index);
    }

    #[test]
    fn test_condition_with_selectivity() {
        let condition = JoinCondition::new("A".to_string(), "B".to_string()).with_selectivity(0.25);

        assert_eq!(condition.selectivity, 0.25);
    }

    #[test]
    fn test_to_decision() {
        let optimizer = create_test_optimizer();
        let tables = create_test_tables();
        let conditions = vec![JoinCondition::new("A".to_string(), "B".to_string())];

        let result = optimizer.optimize_join_order(&tables, &conditions);
        let decision = optimizer.to_decision(&result);

        assert_eq!(decision.join_order.len(), result.order.len());
    }
}
