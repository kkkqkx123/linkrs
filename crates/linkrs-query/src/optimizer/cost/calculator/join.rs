//! Join operator cost models.

use super::CostCalculator;

impl CostCalculator {
    // ==================== Connection Operations ====================

    /// Calculate the cost of hash-based inner joins
    ///
    /// # Parameters
    /// `left_rows`: The number of rows in the left table
    /// `right_rows`: The number of rows in the right table
    pub fn calculate_hash_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        // The cost of constructing a hash table
        let build_cost = left_rows as f64 * self.config.cpu_tuple_cost;
        // Detection cost
        let probe_cost = right_rows as f64 * self.config.cpu_tuple_cost;
        // Hash construction overhead
        let hash_overhead =
            left_rows as f64 * self.config.hash_build_overhead * self.config.cpu_operator_cost;

        build_cost + probe_cost + hash_overhead
    }

    /// Calculating the cost of a hash left join
    pub fn calculate_hash_left_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        // The cost of a left join is similar to that of an inner join, but there may be more output rows.
        self.calculate_hash_join_cost(left_rows, right_rows) * 1.1
    }

    /// Calculate the cost of an inner join (non-hash-based).
    pub fn calculate_inner_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        // Estimation using nested loops
        self.calculate_nested_loop_join_cost(left_rows, right_rows)
    }

    /// Calculate the cost of a left join (non-hash-based).
    pub fn calculate_left_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        self.calculate_nested_loop_join_cost(left_rows, right_rows) * 1.1
    }

    /// Calculate the cost of the cross-connection
    pub fn calculate_cross_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        let output_rows = left_rows as f64 * right_rows as f64;
        output_rows * self.config.cpu_tuple_cost
    }

    /// Calculating the cost of combining nested loops
    pub fn calculate_nested_loop_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        let outer_cost = left_rows as f64 * self.config.cpu_tuple_cost;
        let inner_cost = left_rows as f64 * right_rows as f64 * self.config.cpu_tuple_cost;

        outer_cost + inner_cost
    }

    /// Calculating the cost of a full outer join
    pub fn calculate_full_outer_join_cost(&self, left_rows: u64, right_rows: u64) -> f64 {
        let base_cost = self.calculate_hash_join_cost(left_rows, right_rows);
        base_cost * 1.5 // Full outer joins are more complex.
    }
}
