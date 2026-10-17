//! Graph traversal and graph-algorithm cost models.

use super::super::config::DEFAULT_DEGREE_FALLBACK;
use super::CostCalculator;

impl CostCalculator {
    // ==================== Image traversal operations ====================

    /// Calculate the cost of single-step expansion
    ///
    /// # Parameters
    /// `start_nodes`: Number of starting nodes
    /// `edge_type`: Type of the edge (optional)
    pub fn calculate_expand_cost(
        &self,
        space: &str,
        start_nodes: u64,
        edge_type: Option<&str>,
    ) -> f64 {
        let (avg_degree, is_super_node) = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(space, et)
                .map(|s| {
                    let is_super = s.avg_out_degree > self.config.super_node_threshold as f64;
                    (s.avg_out_degree, is_super)
                })
                .unwrap_or((DEFAULT_DEGREE_FALLBACK, false)),
            None => (DEFAULT_DEGREE_FALLBACK, false),
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
    pub fn calculate_expand_all_cost(
        &self,
        space: &str,
        start_nodes: u64,
        edge_type: Option<&str>,
    ) -> f64 {
        // “ExpandAll” returns more data than “Expand” (including vertex information).
        let base_cost = self.calculate_expand_cost(space, start_nodes, edge_type);
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
        space: &str,
        start_nodes: u64,
        edge_type: Option<&str>,
        steps: u32,
    ) -> f64 {
        let avg_degree = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(space, et)
                .map(|s| (s.avg_out_degree + s.avg_in_degree) / 2.0)
                .unwrap_or(DEFAULT_DEGREE_FALLBACK),
            None => DEFAULT_DEGREE_FALLBACK,
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
    pub fn calculate_get_neighbors_cost(
        &self,
        space: &str,
        start_nodes: u64,
        edge_type: Option<&str>,
    ) -> f64 {
        let avg_degree = match edge_type {
            Some(et) => self
                .stats_manager
                .get_edge_stats(space, et)
                .map(|s| s.avg_out_degree)
                .unwrap_or(DEFAULT_DEGREE_FALLBACK),
            None => DEFAULT_DEGREE_FALLBACK,
        };

        let neighbor_count = (start_nodes as f64 * avg_degree) as u64;
        let lookup_cost = neighbor_count as f64 * self.config.neighbor_lookup_cost;
        let io_cost = self.calculate_io_cost(neighbor_count);

        lookup_cost + io_cost
    }

    /// Calculate the cost of obtaining the vertices.
    ///
    /// Point lookups hit the id index: one index probe plus row materialization
    /// per vid, sharing a single baseline page rather than one random page
    /// per row.
    pub fn calculate_get_vertices_cost(&self, vid_count: u64) -> f64 {
        if vid_count == 0 {
            return 0.0;
        }
        vid_count as f64 * (self.config.cpu_index_tuple_cost + self.config.cpu_tuple_cost)
            + self.config.seq_page_cost
    }

    /// Calculate the cost of the edges.
    ///
    /// Same index-probe model as vertex point lookups.
    pub fn calculate_get_edges_cost(&self, edge_count: u64) -> f64 {
        if edge_count == 0 {
            return 0.0;
        }
        edge_count as f64 * (self.config.cpu_index_tuple_cost + self.config.cpu_tuple_cost)
            + self.config.seq_page_cost
    }

    // ==================== Graph Algorithms ====================

    /// Average branching factor for the given edge types.
    ///
    /// Averages observed average out-degrees; falls back to the configured
    /// default when no statistics are available.
    pub fn average_branching_factor(&self, space: Option<&str>, edge_types: &[String]) -> f64 {
        let Some(space) = space else {
            return self.config.strategy_thresholds.default_branching_factor;
        };
        if edge_types.is_empty() {
            return self.config.strategy_thresholds.default_branching_factor;
        }
        let mut total = 0.0;
        let mut count = 0u32;
        for edge_type in edge_types {
            if let Some(stats) = self.stats_manager.get_edge_stats(space, edge_type) {
                if stats.avg_out_degree > 0.0 {
                    total += stats.avg_out_degree;
                    count += 1;
                }
            }
        }
        if count > 0 {
            total / f64::from(count)
        } else {
            self.config.strategy_thresholds.default_branching_factor
        }
    }

    /// Calculating the cost of the shortest path
    pub fn calculate_shortest_path_cost(&self, start_nodes: u64, max_depth: u32) -> f64 {
        let avg_branching = self.config.strategy_thresholds.default_branching_factor;
        self.calculate_shortest_path_cost_with_branching(start_nodes, max_depth, avg_branching)
    }

    /// Shortest path cost for explicit edge types using observed degrees.
    pub fn calculate_shortest_path_cost_for_edges(
        &self,
        space: Option<&str>,
        edge_types: &[String],
        start_nodes: u64,
        max_depth: u32,
    ) -> f64 {
        let avg_branching = self.average_branching_factor(space, edge_types);
        self.calculate_shortest_path_cost_with_branching(start_nodes, max_depth, avg_branching)
    }

    /// Calculating the cost of the shortest path with an explicit branching factor.
    pub fn calculate_shortest_path_cost_with_branching(
        &self,
        start_nodes: u64,
        max_depth: u32,
        avg_branching: f64,
    ) -> f64 {
        // Complexity estimation based on BFS; single branching-factor source.
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

    /// All paths cost for explicit edge types using observed degrees.
    pub fn calculate_all_paths_cost_for_edges(
        &self,
        space: Option<&str>,
        edge_types: &[String],
        start_nodes: u64,
        max_depth: u32,
    ) -> f64 {
        let base_cost =
            self.calculate_shortest_path_cost_for_edges(space, edge_types, start_nodes, max_depth);
        base_cost * self.config.path_enumeration_factor
    }

    /// Calculating the cost of the shortest path from multiple sources
    pub fn calculate_multi_shortest_path_cost(&self, source_count: u64, max_depth: u32) -> f64 {
        self.calculate_shortest_path_cost(source_count, max_depth) * 1.5
    }
}
