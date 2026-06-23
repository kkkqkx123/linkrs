//! Cost Calculator Module
//!
//! Lightweight cost calculation designed for the characteristics of graph databases
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::cost::{CostCalculator, CostModelConfig};
//! use graphdb::query::optimizer::stats::StatisticsManager;
//! use std::sync::Arc;
//!
//! let stats_manager = Arc::new(StatisticsManager::new());
//! let config = CostModelConfig::default();
//! let calculator = CostCalculator::with_config(stats_manager, config);
//!
// Calculate the scanning cost
//! let scan_cost = calculator.calculate_scan_vertices_cost("Person");
//! ```

use std::sync::Arc;

use crate::core::types::expr::Expression;
use crate::core::value::Value;
use crate::query::optimizer::stats::StatisticsManager;

use super::config::CostModelConfig;

/// Cost Calculator
///
/// Lightweight cost calculation designed for the characteristics of graph databases
#[derive(Debug, Clone)]
pub struct CostCalculator {
    stats_manager: Arc<StatisticsManager>,
    config: CostModelConfig,
}

impl CostCalculator {
    /// Create a new cost calculator (using the default configuration).
    pub fn new(stats_manager: Arc<StatisticsManager>) -> Self {
        Self {
            stats_manager,
            config: CostModelConfig::default(),
        }
    }

    /// Create a new cost calculator (using the specified configuration).
    pub fn with_config(stats_manager: Arc<StatisticsManager>, config: CostModelConfig) -> Self {
        Self {
            stats_manager,
            config,
        }
    }

    /// Obtain the configuration.
    pub fn config(&self) -> &CostModelConfig {
        &self.config
    }

    /// Update the configuration.
    pub fn set_config(&mut self, config: CostModelConfig) {
        self.config = config;
    }

    // ==================== Scanning Operation ====================

    /// Calculate the cost of vertex operations for a full table scan
    ///
    /// Formula: Number of rows × Cost of CPU processing
    pub fn calculate_scan_vertices_cost(&self, tag_name: &str) -> f64 {
        let row_count = self.stats_manager.get_vertex_count(tag_name);
        row_count as f64 * self.config.cpu_tuple_cost
    }

    /// Calculate the cost of scanning the entire table
    ///
    /// Formula: Number of edges × Cost of CPU processing
    pub fn calculate_scan_edges_cost(&self, edge_type: &str) -> f64 {
        let edge_count = self.stats_manager.get_edge_count(edge_type);
        edge_count as f64 * self.config.cpu_tuple_cost
    }

    /// Calculating the cost of index scans
    ///
    /// Formula: Cost of index access + Cost of retrieving data from the table
    ///
    /// # Parameters
    /// `tag_name`: The name of the tag
    /// `property_name`: The name of the property.
    /// - `selectivity`: 选择性（0.0 ~ 1.0）
    pub fn calculate_index_scan_cost(
        &self,
        tag_name: &str,
        _property_name: &str,
        selectivity: f64,
    ) -> f64 {
        let table_rows = self.stats_manager.get_vertex_count(tag_name);
        let matching_rows = (selectivity * table_rows as f64).max(1.0) as u64;

        // Index access cost (sequential I/O)
        let index_pages = (matching_rows / 10).max(1);
        let index_access_cost = index_pages as f64 * self.config.seq_page_cost
            + matching_rows as f64 * self.config.cpu_index_tuple_cost;

        // The cost of retrieving data from the table (random I/O operations)
        let table_access_cost = matching_rows as f64 * self.config.random_page_cost
            + matching_rows as f64 * self.config.cpu_tuple_cost;

        index_access_cost + table_access_cost
    }

    /// Calculating the cost of edge index scanning
    pub fn calculate_edge_index_scan_cost(&self, edge_type: &str, selectivity: f64) -> f64 {
        let edge_count = self.stats_manager.get_edge_count(edge_type);
        let matching_rows = (selectivity * edge_count as f64).max(1.0) as u64;

        let index_pages = (matching_rows / 10).max(1);
        let index_access_cost = index_pages as f64 * self.config.seq_page_cost
            + matching_rows as f64 * self.config.cpu_index_tuple_cost;

        let table_access_cost = matching_rows as f64 * self.config.random_page_cost
            + matching_rows as f64 * self.config.cpu_tuple_cost;

        index_access_cost + table_access_cost
    }

    // ==================== Image traversal operations ====================

    /// Calculate the cost of single-step expansion
    ///
    /// # Parameters
    /// `start_nodes`: Number of starting nodes
    /// `edge_type`: Type of the edge (optional)
    pub fn calculate_expand_cost(&self, start_nodes: u64, edge_type: Option<&str>) -> f64 {
        let (avg_degree, is_super_node) = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(et)
                .map(|s| {
                    let is_super = s.avg_out_degree > self.config.super_node_threshold as f64;
                    (s.avg_out_degree, is_super)
                })
                .unwrap_or((2.0, false)),
            None => (2.0, false), // Default average degree
        };

        let output_rows = (start_nodes as f64 * avg_degree) as u64;

        // IO costs: Reading edge data (cache considerations)
        let io_cost = self.calculate_io_cost(output_rows);
        // CPU cost: The cost associated with edge traversal (more complex than vertex processing).
        let cpu_cost = output_rows as f64 * self.config.edge_traversal_cost;

        let base_cost = io_cost + cpu_cost;

        // Super node penalty for additional costs
        if is_super_node {
            base_cost * self.config.super_node_penalty
        } else {
            base_cost
        }
    }

    /// Calculate the full expansion cost (ExpandAll)
    pub fn calculate_expand_all_cost(&self, start_nodes: u64, edge_type: Option<&str>) -> f64 {
        // “ExpandAll” returns more data than “Expand” (including vertex information).
        let base_cost = self.calculate_expand_cost(start_nodes, edge_type);
        // An additional 50% of the costs is used to obtain vertex information.
        base_cost * 1.5
    }

    /// Calculating the cost of multi-step traversals
    ///
    /// # Parameters
    /// `start_nodes`: The number of starting nodes
    /// `edge_type`: Edge type (optional)
    /// - `steps`: The number of iterations (or steps) in a process.
    pub fn calculate_traverse_cost(
        &self,
        start_nodes: u64,
        edge_type: Option<&str>,
        steps: u32,
    ) -> f64 {
        let avg_degree = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(et)
                .map(|s| (s.avg_out_degree + s.avg_in_degree) / 2.0)
                .unwrap_or(2.0),
            None => 2.0,
        };

        // Calculate the cumulative number of output lines for each step (taking into account the penalty for multiple skips).
        let mut total_cost = 0.0;
        let mut current_rows = start_nodes as f64;

        for step in 0..steps {
            current_rows *= avg_degree;
            // With each additional jump, the cost increases.
            let step_penalty = self.config.multi_hop_penalty.powi(step as i32);
            let step_cost = current_rows * self.config.edge_traversal_cost * step_penalty;
            let io_cost = self.calculate_io_cost(current_rows as u64);
            total_cost += step_cost + io_cost;
        }

        total_cost
    }

    /// Calculating the cost of adding additional vertices
    pub fn calculate_append_vertices_cost(&self, input_rows: u64) -> f64 {
        // For each line, enter additional vertex information.
        input_rows as f64 * self.config.cpu_tuple_cost * 2.0
    }

    /// Calculate the cost of obtaining neighboring nodes
    pub fn calculate_get_neighbors_cost(&self, start_nodes: u64, edge_type: Option<&str>) -> f64 {
        let avg_degree = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(et)
                .map(|s| s.avg_out_degree)
                .unwrap_or(2.0),
            None => 2.0,
        };

        let neighbor_count = (start_nodes as f64 * avg_degree) as u64;
        let lookup_cost = neighbor_count as f64 * self.config.neighbor_lookup_cost;
        let io_cost = self.calculate_io_cost(neighbor_count);

        lookup_cost + io_cost
    }

    /// Calculate the cost of obtaining the vertices.
    pub fn calculate_get_vertices_cost(&self, vid_count: u64) -> f64 {
        vid_count as f64 * self.config.random_page_cost
    }

    /// Calculate the cost of the edges.
    pub fn calculate_get_edges_cost(&self, edge_count: u64) -> f64 {
        edge_count as f64 * self.config.random_page_cost
    }

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

    // ==================== Sorting and Aggregation ====================

    /// Calculating the cost of sorting
    ///
    /// Based on the actual implementation of SortExecutor:
    /// - 小数据量：单线程标准排序 O(n log n)
    /// - Large amounts of data: Scatter-Gather parallel sorting
    /// - 有 LIMIT 且数据量大：使用 Top-N 算法 O(n log k)
    /// - Exceeding the memory threshold: External sorting
    ///
    /// # Parameters
    /// - `input_rows`: 输入行数
    /// - `sort_columns`: The columns to be sorted.
    /// - `limit`: An optional LIMIT value (used for Top-N optimization)
    pub fn calculate_sort_cost(
        &self,
        input_rows: u64,
        sort_columns: usize,
        limit: Option<i64>,
    ) -> f64 {
        if input_rows == 0 {
            return 0.0;
        }

        let rows = input_rows as f64;

        // Check whether Top-N optimization can be used.
        // Refer to SortExecutor: If the amount of data exceeds limit * 10, use the Top-N algorithm.
        if let Some(limit_val) = limit {
            let limit_u = limit_val.max(0) as u64;
            if limit_u > 0 && input_rows > limit_u * 10 {
                // Top-N 算法：使用堆排序，复杂度 O(n log k)
                let k = limit_u as f64;
                return rows
                    * k.log2().max(1.0)
                    * sort_columns as f64
                    * self.config.cpu_operator_cost
                    * self.config.sort_comparison_cost;
            }
        }

        // 标准排序：O(n log n)
        let comparisons = rows * rows.log2().max(1.0);
        let cpu_cost = comparisons
            * sort_columns as f64
            * self.config.cpu_operator_cost
            * self.config.sort_comparison_cost;

        // Determine whether to use external sorting.
        if input_rows > self.config.memory_sort_threshold {
            // External sorting: Temporary files need to be read from and written to.
            let pages = (input_rows / 100).max(1); // Assume there are 100 lines on each page.
            let io_cost = pages as f64 * self.config.external_sort_page_cost * 2.0; // Read and write twice
            cpu_cost + io_cost
        } else {
            cpu_cost
        }
    }

    /// Calculating the cost of implementing the “Limit” feature
    ///
    /// Formula: Number of rows actually processed × Cost of CPU operations
    /// Limit 只需要处理前 N 行，代价与 min(limit, input_rows) 成正比
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
        // 使用堆的复杂度：n × log(k)
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
    /// - `input_rows`: 输入行数
    /// - `agg_functions`: The number of aggregate functions
    /// - `group_by_keys`: The number of keys used in the GROUP BY clause (used to estimate the cost of hash operations)
    pub fn calculate_aggregate_cost(
        &self,
        input_rows: u64,
        agg_functions: usize,
        group_by_keys: usize,
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

        agg_cost + hash_cost
    }

    /// Calculating the cost of deduplication (using a hash table)
    pub fn calculate_dedup_cost(&self, input_rows: u64) -> f64 {
        // The overhead associated with hash insertion and checking
        input_rows as f64 * self.config.cpu_operator_cost * 2.0
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

    // ==================== Graph Algorithms ====================

    /// Calculating the cost of the shortest path
    pub fn calculate_shortest_path_cost(&self, start_nodes: u64, max_depth: u32) -> f64 {
        // Complexity estimation based on BFS (Breadth-First Search)
        let avg_branching = 2.0_f64; // Assume the average branching factor…
        let explored_nodes = start_nodes as f64 * avg_branching.powf(max_depth as f64);
        let traversal_cost = explored_nodes * self.config.edge_traversal_cost;
        let io_cost = self.calculate_io_cost(explored_nodes as u64);

        // Plus the basic expenses.
        traversal_cost + io_cost + self.config.shortest_path_base_cost
    }

    /// Calculate the cost of all paths.
    pub fn calculate_all_paths_cost(&self, start_nodes: u64, max_depth: u32) -> f64 {
        // The complexity of all paths is much higher than that of the shortest path.
        let base_cost = self.calculate_shortest_path_cost(start_nodes, max_depth);
        base_cost * self.config.path_enumeration_factor
    }

    /// Calculating the cost of the shortest path from multiple sources
    pub fn calculate_multi_shortest_path_cost(&self, source_count: u64, max_depth: u32) -> f64 {
        self.calculate_shortest_path_cost(source_count, max_depth) * 1.5
    }

    // ==================== Auxiliary Methods ====================

    /// Statistics Information Manager
    pub fn statistics_manager(&self) -> Arc<StatisticsManager> {
        self.stats_manager.clone()
    }

    /// Estimating the selectivity of tag selection
    pub fn estimate_tag_selectivity(&self, tag_name: &str) -> f64 {
        let vertex_count = self.stats_manager.get_vertex_count(tag_name);
        if vertex_count == 0 {
            1.0
        } else {
            // Simplify the estimation: Assume that the tag distribution is uniform.
            0.1
        }
    }

    /// Estimating the selectivity of edge type choices
    pub fn estimate_edge_selectivity(&self, edge_type: &str) -> f64 {
        let edge_stats = self.stats_manager.get_edge_stats(edge_type);
        match edge_stats {
            Some(stats) if stats.edge_count > 0 => {
                // Estimation based on the number of edges
                (1.0 / (stats.edge_count as f64).sqrt()).clamp(0.001, 1.0)
            }
            _ => 0.1,
        }
    }

    // ==================== Expression Cost Calculation ====================

    /// Calculate expression evaluation cost
    ///
    /// Based on expression complexity (node count) and memory usage
    pub fn calculate_expression_cost(&self, expr: &Expression) -> f64 {
        use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;

        let node_count = expr.node_count() as f64;
        let memory_size = expr.estimate_memory() as f64;
        let memory_factor = memory_size / 1000.0; // Cost per KB

        if expr.is_simple() {
            self.config.simple_expression_cost
        } else {
            node_count * self.config.cpu_operator_cost
                + memory_factor * self.config.memory_byte_cost
        }
    }

    /// Calculate filter cost with expression complexity
    ///
    /// Enhanced version that considers expression evaluation cost
    pub fn calculate_filter_cost_with_expressions(
        &self,
        input_rows: u64,
        conditions: &[Expression],
    ) -> (f64, f64) {
        let base_cost = self.calculate_filter_cost(input_rows, conditions.len());
        let expression_cost: f64 = conditions
            .iter()
            .map(|e| self.calculate_expression_cost(e))
            .sum();
        let total_cost = base_cost + expression_cost * input_rows as f64;
        (total_cost, expression_cost)
    }

    // ==================== Memory-Aware Cost Calculation ====================

    /// Calculate memory-aware cost
    ///
    /// Adds memory usage cost and applies pressure penalty if threshold exceeded
    pub fn calculate_memory_aware_cost(&self, base_cost: f64, memory_usage: usize) -> f64 {
        let memory_cost = memory_usage as f64 * self.config.memory_byte_cost;

        // Check memory pressure
        if memory_usage > self.config.memory_pressure_threshold {
            let pressure_factor =
                memory_usage as f64 / self.config.memory_pressure_threshold as f64;
            let penalty = pressure_factor * self.config.memory_pressure_penalty;
            base_cost * penalty + memory_cost
        } else {
            base_cost + memory_cost
        }
    }

    /// Estimate memory usage for aggregate operations
    pub fn estimate_aggregate_memory(&self, input_rows: u64, group_by_keys: usize) -> usize {
        // Estimate number of groups based on input rows and key count
        let estimated_groups = (input_rows / 2_u64.pow(group_by_keys as u32).max(1)).max(10);
        // Assume average row size of 64 bytes
        estimated_groups as usize * 64
    }

    /// Estimate memory usage for sort operations
    pub fn estimate_sort_memory(&self, input_rows: u64, _sort_columns: usize) -> usize {
        // Sorting needs to buffer all input rows
        input_rows as usize * 64 // Assume 64 bytes per row
    }

    /// Estimate memory usage for hash join operations
    pub fn estimate_hash_join_memory(&self, left_rows: u64) -> usize {
        // Hash table needs to store the smaller (left) table
        left_rows as usize * 64 // Assume 64 bytes per row
    }

    /// Calculate aggregate cost with memory awareness
    pub fn calculate_aggregate_cost_enhanced(
        &self,
        input_rows: u64,
        agg_functions: usize,
        group_by_keys: usize,
    ) -> (f64, usize) {
        let base_cost = self.calculate_aggregate_cost(input_rows, agg_functions, group_by_keys);
        let memory_usage = self.estimate_aggregate_memory(input_rows, group_by_keys);
        let total_cost = self.calculate_memory_aware_cost(base_cost, memory_usage);
        (total_cost, memory_usage)
    }

    /// Calculate sort cost with memory awareness
    pub fn calculate_sort_cost_enhanced(
        &self,
        input_rows: u64,
        sort_columns: usize,
        limit: Option<i64>,
    ) -> (f64, usize) {
        let base_cost = self.calculate_sort_cost(input_rows, sort_columns, limit);
        let memory_usage = self.estimate_sort_memory(input_rows, sort_columns);
        let total_cost = self.calculate_memory_aware_cost(base_cost, memory_usage);
        (total_cost, memory_usage)
    }

    /// Calculate hash join cost with memory awareness
    pub fn calculate_hash_join_cost_enhanced(
        &self,
        left_rows: u64,
        right_rows: u64,
    ) -> (f64, usize) {
        let base_cost = self.calculate_hash_join_cost(left_rows, right_rows);
        let memory_usage = self.estimate_hash_join_memory(left_rows);
        let total_cost = self.calculate_memory_aware_cost(base_cost, memory_usage);
        (total_cost, memory_usage)
    }

    // ==================== Data Type Cost Factors ====================

    /// Get cost factor for a value type
    ///
    /// Returns the appropriate cost factor based on value type complexity
    pub fn get_type_cost_factor(&self, value: &Value) -> f64 {
        match value {
            // Fixed-size types
            Value::Empty
            | Value::Null(_)
            | Value::Bool(_)
            | Value::SmallInt(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Double(_) => self.config.fixed_type_cost_factor,

            // Variable-length types
            Value::String(_) | Value::FixedString { .. } | Value::Blob(_) => {
                self.config.variable_type_cost_factor
            }

            // Complex types
            Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::Decimal128(_)
            | Value::Geography(_)
            | Value::Date(_)
            | Value::Time(_)
            | Value::DateTime(_) => self.config.complex_type_cost_factor,

            // Graph types
            Value::Vertex(_) | Value::Edge(_) | Value::Path(_) => {
                self.config.graph_type_cost_factor
            }

            // DataSet
            Value::DataSet(_) => self.config.complex_type_cost_factor * 1.25,

            // Vector type
            Value::Vector(_) => self.config.complex_type_cost_factor * 1.3,

            // JSON types
            Value::Json(_) | Value::JsonB(_) => self.config.complex_type_cost_factor * 1.2,

            // UUID type (fixed size, 16 bytes)
            Value::Uuid(_) => self.config.fixed_type_cost_factor,

            // Interval type
            Value::Interval(_) => self.config.fixed_type_cost_factor,
        }
    }

    // ==================== Calculation of the Cost of Cache-Aware I/O Operations ====================

    /// Calculating the cost of I/O operations (taking caching into account)
    ///
    /// Adjust the I/O overhead based on the size of the effective cache:
    /// - If the number of data pages accessed is < effective_cache_pages: Most of the pages are already in the cache.
    /// - Otherwise: Some operations require disk I/O (input/output).
    fn calculate_io_cost(&self, rows: u64) -> f64 {
        // Assume there are 100 lines on each page.
        let pages = (rows / 100).max(1);

        if pages <= self.config.effective_cache_pages {
            // The data may be in the cache.
            pages as f64 * self.config.seq_page_cost * self.config.cache_hit_cost_factor
        } else {
            // Some data needs to be read from the disk.
            let cached_pages = self.config.effective_cache_pages;
            let disk_pages = pages - cached_pages;

            let cached_cost =
                cached_pages as f64 * self.config.seq_page_cost * self.config.cache_hit_cost_factor;
            let disk_cost = disk_pages as f64 * self.config.seq_page_cost;

            cached_cost + disk_cost
        }
    }
}

impl Default for CostCalculator {
    fn default() -> Self {
        Self {
            stats_manager: Arc::new(StatisticsManager::new()),
            config: CostModelConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_scan_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        // When no statistical information is available, a value of 0 should be returned.
        let cost = calculator.calculate_scan_vertices_cost("NonExistent");
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_calculate_filter_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let cost = calculator.calculate_filter_cost(1000, 3);
        assert!(cost > 0.0);
        // 1000 * 3 * 0.0025 = 7.5
        assert_eq!(cost, 7.5);
    }

    #[test]
    fn test_calculate_hash_join_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let cost = calculator.calculate_hash_join_cost(100, 200);
        assert!(cost > 0.0);
        // (100 + 200) * 0.01 + 100 * 0.1 * 0.0025 = 3.0 + 0.025 = 3.025
        assert_eq!(cost, 3.025);
    }

    #[test]
    fn test_calculate_sort_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        // Test standard sorting (with no limit)
        let cost = calculator.calculate_sort_cost(1000, 2, None);
        assert!(cost > 0.0);

        // An empty input should return 0.
        let zero_cost = calculator.calculate_sort_cost(0, 2, None);
        assert_eq!(zero_cost, 0.0);

        // Testing the Top-N optimization (data volume > limit * 10)
        let topn_cost = calculator.calculate_sort_cost(1000, 2, Some(50));
        // The Top-N algorithm should be cheaper than the full sorting algorithm.
        assert!(topn_cost < cost);

        // Testing the small limit (which does not trigger the Top-N result).
        let small_limit_cost = calculator.calculate_sort_cost(1000, 2, Some(200));
        // The “small limit” should be sorted using the standard sorting method.
        assert!(small_limit_cost >= cost * 0.99 && small_limit_cost <= cost * 1.01);
    }

    #[test]
    fn test_calculate_topn_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let cost = calculator.calculate_topn_cost(10000, 10);
        assert!(cost > 0.0);
        // 10000 * log2(10) * 0.0025 ≈ 83.05
        assert!(cost > 80.0 && cost < 85.0);
    }

    #[test]
    fn test_with_config() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let config = CostModelConfig::for_ssd();
        let calculator = CostCalculator::with_config(stats_manager, config);

        assert_eq!(calculator.config().random_page_cost, 1.1);
    }

    #[test]
    fn test_memory_aware_cost() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        // Test normal memory usage (below threshold)
        let base_cost = 100.0;
        let normal_memory = 1024 * 1024; // 1MB
        let normal_cost = calculator.calculate_memory_aware_cost(base_cost, normal_memory);
        // Memory cost for 1MB is: 1,048,576 * 0.0001 = 104.8576
        // So total should be around 204.86
        assert!(normal_cost > base_cost); // Should add memory cost

        // Test high memory usage (above threshold)
        let high_memory = 200 * 1024 * 1024; // 200MB (above 100MB threshold)
        let high_cost = calculator.calculate_memory_aware_cost(base_cost, high_memory);
        assert!(high_cost > normal_cost); // Should have penalty
    }

    #[test]
    fn test_estimate_aggregate_memory() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        // Test with no group by keys
        let mem_no_groups = calculator.estimate_aggregate_memory(1000, 0);
        assert!(mem_no_groups > 0);

        // Test with multiple group by keys
        let mem_with_groups = calculator.estimate_aggregate_memory(10000, 3);
        assert!(mem_with_groups > 0);
    }

    #[test]
    fn test_estimate_sort_memory() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let memory = calculator.estimate_sort_memory(1000, 2);
        assert_eq!(memory, 1000 * 64); // 1000 rows * 64 bytes
    }

    #[test]
    fn test_estimate_hash_join_memory() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let memory = calculator.estimate_hash_join_memory(500);
        assert_eq!(memory, 500 * 64); // 500 rows * 64 bytes
    }

    #[test]
    fn test_calculate_aggregate_cost_enhanced() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let (cost, memory) = calculator.calculate_aggregate_cost_enhanced(1000, 2, 1);
        assert!(cost > 0.0);
        assert!(memory > 0);
    }

    #[test]
    fn test_calculate_sort_cost_enhanced() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let (cost, memory) = calculator.calculate_sort_cost_enhanced(1000, 2, None);
        assert!(cost > 0.0);
        assert_eq!(memory, 1000 * 64);
    }

    #[test]
    fn test_calculate_hash_join_cost_enhanced() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        let (cost, memory) = calculator.calculate_hash_join_cost_enhanced(100, 200);
        assert!(cost > 0.0);
        assert_eq!(memory, 100 * 64);
    }

    #[test]
    fn test_get_type_cost_factor() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let calculator = CostCalculator::new(stats_manager);

        // Fixed-size types
        assert_eq!(
            calculator.get_type_cost_factor(&Value::Int(42)),
            calculator.config.fixed_type_cost_factor
        );
        assert_eq!(
            calculator.get_type_cost_factor(&Value::Bool(true)),
            calculator.config.fixed_type_cost_factor
        );

        // Variable-length types
        assert_eq!(
            calculator.get_type_cost_factor(&Value::String("test".to_string())),
            calculator.config.variable_type_cost_factor
        );

        // Complex types
        assert_eq!(
            calculator
                .get_type_cost_factor(&Value::List(Box::<crate::core::value::List>::default())),
            calculator.config.complex_type_cost_factor
        );

        // Graph types
        use crate::core::vertex_edge_path::Vertex;
        let vertex = Vertex::with_vid(crate::core::types::VertexId::from_int64(1));
        assert_eq!(
            calculator.get_type_cost_factor(&Value::Vertex(Box::new(vertex))),
            calculator.config.graph_type_cost_factor
        );
    }
}
