//! Scan operator cost models.

use super::super::config::ROWS_PER_PAGE;
use super::CostCalculator;

impl CostCalculator {
    // ==================== Scanning Operation ====================

    /// Calculate the cost of vertex operations for a full table scan
    ///
    /// Formula: rows x CPU cost + cache-aware sequential I/O for the pages read.
    /// Page count derives from allocated slots (holes included) so tables
    /// with heavy deletes cost their physical pages; CPU still counts live
    /// rows only.
    pub fn calculate_scan_vertices_cost(&self, space: &str, tag_name: &str) -> f64 {
        let row_count = self.stats_manager.get_vertex_count(space, tag_name);
        if row_count == 0 {
            return 0.0;
        }
        let allocated = self
            .stats_manager
            .get_tag_stats(space, tag_name)
            .and_then(|s| s.allocated_slots);
        row_count as f64 * self.config.cpu_tuple_cost
            + self.calculate_io_cost_with_allocated(row_count, allocated)
    }

    /// Calculate the cost of scanning the entire table
    ///
    /// Formula: edges x CPU cost + cache-aware sequential I/O for the pages read.
    /// Page count derives from allocated slots when the snapshot exists.
    pub fn calculate_scan_edges_cost(&self, space: &str, edge_type: &str) -> f64 {
        let edge_count = self.stats_manager.get_edge_count(space, edge_type);
        if edge_count == 0 {
            return 0.0;
        }
        let allocated = self
            .stats_manager
            .get_edge_stats(space, edge_type)
            .and_then(|s| s.allocated_slots);
        edge_count as f64 * self.config.cpu_tuple_cost
            + self.calculate_io_cost_with_allocated(edge_count, allocated)
    }

    /// Calculating the cost of index scans
    ///
    /// Formula: Cost of index access + Cost of retrieving data from the table
    ///
    /// # Parameters
    /// `tag_name`: The name of the tag
    /// `property_name`: The name of the property.
    /// - `selectivity`: The selectivity (0.0 ~ 1.0)
    pub fn calculate_index_scan_cost(
        &self,
        space: &str,
        tag_name: &str,
        _property_name: &str,
        selectivity: f64,
    ) -> f64 {
        let table_rows = self.stats_manager.get_vertex_count(space, tag_name);
        let matching_rows = (selectivity * table_rows as f64).max(1.0) as u64;

        // Index access cost (sequential I/O); single page/row source.
        let index_pages = (matching_rows / ROWS_PER_PAGE).max(1);
        let index_access_cost = index_pages as f64 * self.config.seq_page_cost
            + matching_rows as f64 * self.config.cpu_index_tuple_cost;

        // The cost of retrieving data from the table (random I/O operations).
        // Holes spread live rows over more heap pages, so the revisit cost
        // grows with the hole rate (bounded by 2x since the rate is below 1).
        let hole_rate = self
            .stats_manager
            .get_tag_stats(space, tag_name)
            .map(|s| s.hole_rate())
            .unwrap_or(0.0);
        let table_access_cost =
            matching_rows as f64 * self.config.random_page_cost * (1.0 + hole_rate)
                + matching_rows as f64 * self.config.cpu_tuple_cost;

        index_access_cost + table_access_cost
    }

    /// Calculating the cost of edge index scanning
    pub fn calculate_edge_index_scan_cost(
        &self,
        space: &str,
        edge_type: &str,
        selectivity: f64,
    ) -> f64 {
        let edge_count = self.stats_manager.get_edge_count(space, edge_type);
        let matching_rows = (selectivity * edge_count as f64).max(1.0) as u64;

        let index_pages = (matching_rows / ROWS_PER_PAGE).max(1);
        let index_access_cost = index_pages as f64 * self.config.seq_page_cost
            + matching_rows as f64 * self.config.cpu_index_tuple_cost;

        let hole_rate = self
            .stats_manager
            .get_edge_stats(space, edge_type)
            .map(|s| s.hole_rate())
            .unwrap_or(0.0);
        let table_access_cost =
            matching_rows as f64 * self.config.random_page_cost * (1.0 + hole_rate)
                + matching_rows as f64 * self.config.cpu_tuple_cost;

        index_access_cost + table_access_cost
    }
}
