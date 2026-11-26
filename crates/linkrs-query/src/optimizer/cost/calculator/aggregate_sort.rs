//! Filtering, projection, sorting, aggregation and set-operation cost models.

use super::super::config::TOPN_RATIO;
use super::CostCalculator;

impl CostCalculator {
    // ==================== Filtering and Projection ====================

    /// Calculating the cost of filtering
    ///
    /// Formula: Number of input rows × Number of conditions × Cost of each operator
    ///
    /// # Parameters
    /// `input_rows`: The number of input rows
    /// `condition_count`: Number of conditions
    pub fn calculate_filter_cost(&self, input_rows: u64, condition_count: usize) -> f64 {
        input_rows as f64 * condition_count as f64 * self.config.cpu_operator_cost
    }

    /// Calculating the cost of projection
    ///
    /// # Parameters
    /// `input_rows`: Number of input rows
    /// - `columns`: The number of columns to be projected.
    pub fn calculate_project_cost(&self, input_rows: u64, columns: usize) -> f64 {
        input_rows as f64 * columns as f64 * self.config.cpu_operator_cost
    }

    // ==================== Sorting and Aggregation ====================

    /// Calculating the cost of sorting
    ///
    /// Based on the actual implementation of SortExecutor:
    /// - Small data volume: single-threaded standard sort O(n log n)
    /// - Large amounts of data: Scatter-Gather parallel sorting
    /// - With LIMIT and large data volume: Top-N algorithm O(n log k)
    /// - Exceeding the memory threshold: External sorting
    ///
    /// # Parameters
    /// - `input_rows`: The number of input rows
    /// - `sort_columns`: The columns to be sorted.
    /// - `limit`: An optional LIMIT value (used for Top-N optimization)
    pub fn calculate_sort_cost(
        &self,
        input_rows: u64,
        sort_columns: usize,
        limit: Option<i64>,
    ) -> f64 {
        self.calculate_sort_cost_with_budget(
            input_rows,
            sort_columns,
            limit,
            self.config.memory_pressure_threshold as u64,
        )
    }

    /// Sorting cost priced against an explicit operator memory budget.
    pub fn calculate_sort_cost_with_budget(
        &self,
        input_rows: u64,
        sort_columns: usize,
        limit: Option<i64>,
        budget_bytes: u64,
    ) -> f64 {
        if input_rows == 0 {
            return 0.0;
        }

        let rows = input_rows as f64;

        // Check whether Top-N optimization can be used.
        // Single TopN rule: input exceeds limit * TOPN_RATIO.
        if let Some(limit_val) = limit {
            let limit_u = limit_val.max(0) as u64;
            if limit_u > 0 && input_rows > limit_u.saturating_mul(TOPN_RATIO) {
                // Top-N algorithm: uses a heap sort, complexity O(n log k)
                let k = limit_u as f64;
                return rows
                    * k.log2().max(1.0)
                    * sort_columns as f64
                    * self.config.cpu_operator_cost
                    * self.config.sort_comparison_cost;
            }
        }

        // Standard sort: O(n log n)
        let comparisons = rows * rows.log2().max(1.0);
        let cpu_cost = comparisons
            * sort_columns as f64
            * self.config.cpu_operator_cost
            * self.config.sort_comparison_cost;

        // External sorting is priced by the estimated working set that does
        // not fit the operator budget, not by a row-count threshold alone.
        let spill = self.estimate_spill_cost(input_rows, budget_bytes, 2);
        cpu_cost + spill.io_cost
    }

    /// Calculating the cost of implementing the “Limit” feature
    ///
    /// Formula: Number of rows actually processed × Cost of CPU operations
    /// LIMIT only needs to process the first N rows; the cost is proportional to min(limit, input_rows)
    pub fn calculate_limit_cost(&self, input_rows: u64, limit: i64) -> f64 {
        let rows_to_process = (limit.max(0) as u64).min(input_rows);
        rows_to_process as f64 * self.config.cpu_operator_cost * 0.5
    }

    /// Calculating the cost of obtaining the TopN results (using a priority queue)
    ///
    /// More efficient than full sorting; implemented using a heap.
    pub fn calculate_topn_cost(&self, input_rows: u64, limit: i64) -> f64 {
        let n = input_rows as f64;
        let k = limit as f64;
        // Heap complexity: n × log(k)
        n * k.log2().max(1.0) * self.config.cpu_operator_cost
    }

    /// Calculating the cost of aggregation
    ///
    /// Based on the actual implementation of AggregateExecutor:
    /// Use a HashMap to store the group status.
    /// It is necessary to calculate the grouping key (evaluation of the expression).
    /// Each aggregate function requires an update of its status.
    ///
    /// # Parameters
    /// - `input_rows`: The number of input rows
    /// - `agg_functions`: The number of aggregate functions
    /// - `group_by_keys`: The number of keys used in the GROUP BY clause (used to estimate the cost of hash operations)
    pub fn calculate_aggregate_cost(
        &self,
        input_rows: u64,
        agg_functions: usize,
        group_by_keys: usize,
    ) -> f64 {
        self.calculate_aggregate_cost_with_budget(
            input_rows,
            agg_functions,
            group_by_keys,
            self.config.memory_pressure_threshold as u64,
        )
    }

    /// Aggregate cost priced against an explicit operator memory budget.
    pub fn calculate_aggregate_cost_with_budget(
        &self,
        input_rows: u64,
        agg_functions: usize,
        group_by_keys: usize,
        budget_bytes: u64,
    ) -> f64 {
        // The computational cost of basic aggregate functions
        let agg_cost = input_rows as f64 * agg_functions as f64 * self.config.cpu_operator_cost;

        // Cost of hash table operations (insertion, search)
        // For each input line, it is necessary to calculate the group key and perform a hashing operation.
        let hash_cost = if group_by_keys > 0 {
            input_rows as f64 * group_by_keys as f64 * self.config.cpu_operator_cost * 2.0
        } else {
            0.0
        };

        let spill = if group_by_keys > 0 {
            self.estimate_spill_cost(input_rows, budget_bytes, 2)
        } else {
            Default::default()
        };
        agg_cost + hash_cost + spill.io_cost
    }

    /// Calculating the cost of deduplication (using a hash table)
    pub fn calculate_dedup_cost(&self, input_rows: u64) -> f64 {
        // The overhead associated with hash insertion and checking
        let cpu = input_rows as f64 * self.config.cpu_operator_cost * 2.0;
        let spill =
            self.estimate_spill_cost(input_rows, self.config.memory_pressure_threshold as u64, 2);
        cpu + spill.io_cost
    }

    // ==================== Data Processing and Set Operations ====================

    /// Calculating the cost of a Union operation
    pub fn calculate_union_cost(&self, left_rows: u64, right_rows: u64, distinct: bool) -> f64 {
        let base_cost = (left_rows + right_rows) as f64 * self.config.cpu_tuple_cost;
        if distinct {
            // The text needs to be deduplicated (that is, duplicate entries should be removed).
            base_cost + self.calculate_dedup_cost(left_rows + right_rows)
        } else {
            base_cost
        }
    }

    /// Calculate the cost of “Minus”.
    pub fn calculate_minus_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        let base_cost = (left_rows + right_rows) as f64 * self.config.cpu_tuple_cost;
        // Hash set operations are required.
        let set_op_cost = right_rows as f64 * self.config.cpu_operator_cost;
        base_cost + set_op_cost
    }

    /// Calculate the Intersect cost
    pub fn calculate_intersect_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        let base_cost = (left_rows + right_rows) as f64 * self.config.cpu_tuple_cost;
        let set_op_cost = left_rows.min(right_rows) as f64 * self.config.cpu_operator_cost;
        base_cost + set_op_cost
    }

    /// Calculate the cost of “Unwind”
    pub fn calculate_unwind_cost(&self, input_rows: u64, avg_list_size: f64) -> f64 {
        let output_rows = input_rows as f64 * avg_list_size;
        output_rows * self.config.cpu_tuple_cost
    }

    /// Calculating the cost of data collection
    pub fn calculate_data_collect_cost(&self, input_rows: u64) -> f64 {
        input_rows as f64 * self.config.cpu_tuple_cost
    }

    /// Calculating the cost of sampling
    pub fn calculate_sample_cost(&self, input_rows: u64) -> f64 {
        // Sampling requires traversing the data.
        input_rows as f64 * self.config.cpu_operator_cost
    }

    // ==================== Control Flow Nodes ====================

    /// Calculating the cost of loop iterations
    ///
    /// # Parameters
    /// `body_cost`: The cost of the loop body
    /// `iterations`: An estimate of the number of iterations required.
    pub fn calculate_loop_cost(&self, body_cost: f64, iterations: u32) -> f64 {
        body_cost * iterations as f64
    }

    /// Calculating the cost of selecting a node
    pub fn calculate_select_cost(&self, input_rows: u64, branch_count: usize) -> f64 {
        input_rows as f64 * branch_count as f64 * self.config.cpu_operator_cost
    }

    /// Calculating the cost of transparent nodes
    pub fn calculate_pass_through_cost(&self, input_rows: u64) -> f64 {
        input_rows as f64 * self.config.cpu_operator_cost * 0.1
    }
}
