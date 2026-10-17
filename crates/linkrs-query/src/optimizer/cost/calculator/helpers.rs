//! Auxiliary cost helpers: statistics access, expression and memory
//! estimation, type cost factors and cache-aware I/O.

use std::sync::Arc;

use crate::optimizer::stats::{StatisticsManager, StatsView};
use linkrs_core::types::expr::Expression;
use linkrs_core::value::Value;

use super::super::config::ROWS_PER_PAGE;
use super::CostCalculator;

impl CostCalculator {
    // ==================== Auxiliary Methods ====================

    /// Statistics Information Manager
    pub fn statistics_manager(&self) -> Arc<StatisticsManager> {
        self.stats_manager.clone()
    }

    /// Create a space-scoped statistics view over this calculator's statistics.
    ///
    /// The view binds the space name to the underlying statistics manager, so
    /// estimators receive a single consistent source of statistics.
    pub fn stats_view<'a>(&'a self, space: Option<&'a str>) -> StatsView<'a> {
        StatsView::new(&self.stats_manager, space)
    }

    /// Estimating the selectivity of tag selection
    ///
    /// Share of the tag within its space; falls back to 0.1 when the space
    /// census is unavailable so callers share one code path.
    pub fn estimate_tag_selectivity(&self, space: &str, tag_name: &str) -> f64 {
        let vertex_count = self.stats_manager.get_vertex_count(space, tag_name);
        if vertex_count == 0 {
            return 1.0;
        }
        let mut total: u64 = 0;
        for tag in self.stats_manager.get_all_tags() {
            total += self.stats_manager.get_vertex_count(space, &tag);
        }
        if total == 0 {
            0.1
        } else {
            (vertex_count as f64 / total as f64).clamp(0.001, 1.0)
        }
    }

    /// Estimating the selectivity of edge type choices
    ///
    /// Share of the edge type within its space; same fallback discipline.
    pub fn estimate_edge_selectivity(&self, space: &str, edge_type: &str) -> f64 {
        let edge_stats = self.stats_manager.get_edge_stats(space, edge_type);
        let count = edge_stats.map(|s| s.edge_count).unwrap_or(0);
        if count == 0 {
            return 0.1;
        }
        let mut total: u64 = 0;
        for et in self.stats_manager.get_all_edge_types() {
            total += self.stats_manager.get_edge_count(space, &et);
        }
        if total == 0 {
            0.1
        } else {
            (count as f64 / total as f64).clamp(0.001, 1.0)
        }
    }

    // ==================== Expression Cost Calculation ====================

    /// Calculate expression evaluation cost
    ///
    /// Based on expression complexity (node count) and memory usage
    pub fn calculate_expression_cost(&self, expr: &Expression) -> f64 {
        use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;

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
    /// Single row-width source (`CostModelConfig::row_width_bytes`) and single
    /// group-count source (`ndv::estimate_group_count`).
    pub fn estimate_aggregate_memory(&self, input_rows: u64, group_by_keys: usize) -> usize {
        let estimated_groups =
            crate::optimizer::cost_based::ndv::estimate_group_count(input_rows, group_by_keys);
        estimated_groups as usize * self.config.row_width_bytes
    }

    /// Estimate memory usage for sort operations
    pub fn estimate_sort_memory(&self, input_rows: u64, _sort_columns: usize) -> usize {
        // Sorting needs to buffer all input rows; width from the shared config.
        input_rows as usize * self.config.row_width_bytes
    }

    /// Estimate memory usage for hash join operations
    pub fn estimate_hash_join_memory(&self, left_rows: u64) -> usize {
        // Hash table needs to store the smaller (left) table.
        left_rows as usize * self.config.row_width_bytes
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
            Value::String(_) | Value::FixedString(_) | Value::Blob(_) => {
                self.config.variable_type_cost_factor
            }

            // Complex types
            Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::Struct(_)
            | Value::Array(_)
            | Value::Decimal128(_)
            | Value::Geography(_)
            | Value::Date(_)
            | Value::Time(_)
            | Value::DateTime(_) => self.config.complex_type_cost_factor,

            // Graph types
            Value::Vertex(_) | Value::Edge(_) | Value::EdgeHeader(_) | Value::Path(_) => {
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

            // Lightweight ID references (fixed size)
            Value::VertexId(_) | Value::EdgeId(_) => self.config.fixed_type_cost_factor,
        }
    }

    // ==================== Calculation of the Cost of Cache-Aware I/O Operations ====================

    /// Calculating the cost of I/O operations (taking caching into account)
    ///
    /// Adjust the I/O overhead based on the size of the effective cache:
    /// - If the number of data pages accessed is < effective_cache_pages: Most of the pages are already in the cache.
    /// - Otherwise: Some operations require disk I/O (input/output).
    pub(super) fn calculate_io_cost(&self, rows: u64) -> f64 {
        self.calculate_io_cost_with_allocated(rows, None)
    }

    /// Cache-aware I/O cost with physical sizing.
    ///
    /// `live_rows` sizes nothing by itself; page count derives from
    /// `allocated_slots` (holes included) when the snapshot exists, else
    /// from live rows. CPU costing stays with callers on live rows.
    pub(super) fn calculate_io_cost_with_allocated(
        &self,
        live_rows: u64,
        allocated_slots: Option<u64>,
    ) -> f64 {
        // Single page/row source: ROWS_PER_PAGE rows per page.
        // `effective_cache_pages` counts pages; callers pass rows.
        // Snapshots below the live count are inconsistent (shard skew);
        // fall back to live rows rather than shrinking the page count.
        let sized = allocated_slots
            .filter(|allocated| *allocated >= live_rows)
            .unwrap_or(live_rows);
        let pages = (sized / ROWS_PER_PAGE).max(1);

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
