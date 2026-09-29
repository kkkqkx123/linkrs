//! Border Type Statistics Module
//!
//! Provide statistical information at the edge type level, which is used by the query optimizer to estimate the cost of traversing the data.

/// Hotspot vertex information
#[derive(Debug, Clone)]
pub struct HotVertexInfo {
    /// Vertex ID
    pub vertex_id: i64,
    /// Out-degree
    pub out_degree: u64,
    /// In-degree
    pub in_degree: u64,
}

/// Grade of inclination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkewnessLevel {
    /// No tilt
    None,
    /// Slight inclination
    Mild,
    /// Moderate inclination
    Moderate,
    /// Severe tilt
    Severe,
}

/// Edge type statistics information
#[derive(Debug, Clone)]
pub struct EdgeTypeStatistics {
    /// Edge Type Name
    pub edge_type: String,
    /// Total number of edges
    pub edge_count: u64,
    /// Allocated slots including deleted-but-unreclaimed holes, when the
    /// storage engine exposes a table cardinality snapshot. `None` means
    /// unknown and costs fall back to the live count (zero holes).
    pub allocated_slots: Option<u64>,
    /// Average frequency of use
    pub avg_out_degree: f64,
    /// Average Indegree
    pub avg_in_degree: f64,
    /// Maximum Outdegree
    pub max_out_degree: u64,
    /// Maximum In-degree
    pub max_in_degree: u64,
    /// The number of unique source vertices
    pub unique_src_vertices: u64,
    /// Outlier standard deviation (a measure of the degree of dispersion of a distribution)
    pub out_degree_std_dev: f64,
    /// In-degree standard deviation
    pub in_degree_std_dev: f64,
    /// Gini coefficient (ranging from 0 to 1; the higher the value, the more unequal the distribution)
    pub degree_gini_coefficient: f64,
    /// List of the top K vertices (vertices with the highest degree)
    pub hot_vertices: Vec<HotVertexInfo>,
    /// Space this statistics was collected for (`None` = default).
    pub space: Option<String>,
    /// Schema version at collection time (used for staleness checks).
    pub schema_version: Option<u64>,
}

impl EdgeTypeStatistics {
    /// Create new statistical information for edge types.
    pub fn new(edge_type: String) -> Self {
        Self {
            edge_type,
            edge_count: 0,
            allocated_slots: None,
            avg_out_degree: 0.0,
            avg_in_degree: 0.0,
            max_out_degree: 0,
            max_in_degree: 0,
            unique_src_vertices: 0,
            out_degree_std_dev: 0.0,
            in_degree_std_dev: 0.0,
            degree_gini_coefficient: 0.0,
            hot_vertices: Vec::new(),
            space: None,
            schema_version: None,
        }
    }

    /// Attach space and schema version provenance.
    pub fn with_version(mut self, space: String, schema_version: u64) -> Self {
        self.space = Some(space);
        self.schema_version = Some(schema_version);
        self
    }

    /// Estimate the cost of expansion
    pub fn estimate_expand_cost(&self, start_nodes: u64) -> f64 {
        start_nodes as f64 * self.avg_out_degree
    }

    /// Hole rate `1 - live / allocated`, clamped to `[0, 1)`.
    ///
    /// Same contract as the tag-level helper: unknown or inconsistent
    /// snapshots report zero.
    pub fn hole_rate(&self) -> f64 {
        match self.allocated_slots {
            Some(allocated)
                if allocated > 0 && allocated > self.edge_count && self.edge_count > 0 =>
            {
                1.0 - (self.edge_count as f64 / allocated as f64)
            }
            _ => 0.0,
        }
    }

    /// Fill the allocated slot count from a storage table snapshot.
    /// Snapshots that carry no rows leave the field unknown.
    pub fn apply_table_snapshot(
        &mut self,
        snapshot: Option<&crate::storage::stats_reader::TableCardinalitySnapshot>,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        if snapshot.allocated_slots > 0 {
            self.allocated_slots = Some(snapshot.allocated_slots);
        }
    }

    /// Determine whether there is a significant inclination.
    pub fn is_heavily_skewed(&self) -> bool {
        use crate::optimizer::cost::config::{SKEW_MAX_DEGREE_RATIO, SKEW_SEVERE_GINI};
        self.degree_gini_coefficient > SKEW_SEVERE_GINI
            || self.max_out_degree as f64 > self.avg_out_degree * SKEW_MAX_DEGREE_RATIO
    }

    /// Obtaining the inclination level
    pub fn skewness_level(&self) -> SkewnessLevel {
        use crate::optimizer::cost::config::{
            SKEW_MILD_GINI, SKEW_MODERATE_GINI, SKEW_SEVERE_GINI,
        };
        match self.degree_gini_coefficient {
            g if g > SKEW_SEVERE_GINI => SkewnessLevel::Severe,
            g if g > SKEW_MODERATE_GINI => SkewnessLevel::Moderate,
            g if g > SKEW_MILD_GINI => SkewnessLevel::Mild,
            _ => SkewnessLevel::None,
        }
    }

    /// Shared skew penalty so cost and row estimation use one definition.
    pub fn skew_penalty(&self) -> f64 {
        use crate::optimizer::cost::config::{
            SKEW_MILD_PENALTY, SKEW_MODERATE_PENALTY, SKEW_SEVERE_PENALTY,
        };
        match self.skewness_level() {
            SkewnessLevel::Severe => SKEW_SEVERE_PENALTY,
            SkewnessLevel::Moderate => SKEW_MODERATE_PENALTY,
            SkewnessLevel::Mild => SKEW_MILD_PENALTY,
            SkewnessLevel::None => 1.0,
        }
    }

    /// Shared conservative row factor for skewed expansion.
    pub fn skew_row_factor(&self) -> f64 {
        use crate::optimizer::cost::config::{
            SKEW_MILD_ROW_FACTOR, SKEW_MODERATE_ROW_FACTOR, SKEW_SEVERE_ROW_FACTOR,
        };
        match self.skewness_level() {
            SkewnessLevel::Severe => SKEW_SEVERE_ROW_FACTOR,
            SkewnessLevel::Moderate => SKEW_MODERATE_ROW_FACTOR,
            SkewnessLevel::Mild => SKEW_MILD_ROW_FACTOR,
            SkewnessLevel::None => 1.0,
        }
    }

    /// Calculate the cost of tilt perception (use a more conservative estimate for tilted data)
    pub fn calculate_skewed_expand_cost(&self, start_nodes: u64) -> f64 {
        let base_cost = self.estimate_expand_cost(start_nodes);
        base_cost * self.skew_penalty()
    }

    /// Determine whether it contains a hot spot vertex.
    pub fn has_hot_vertices(&self) -> bool {
        !self.hot_vertices.is_empty()
    }

    /// Obtain the number of hot vertices
    pub fn hot_vertex_count(&self) -> usize {
        self.hot_vertices.len()
    }
}

impl Default for EdgeTypeStatistics {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hole_rate_clamps_unknown_and_inconsistent() {
        let mut stats = EdgeTypeStatistics::new("knows".to_string());
        assert_eq!(stats.hole_rate(), 0.0);

        stats.edge_count = 1000;
        stats.allocated_slots = Some(4000);
        assert!((stats.hole_rate() - 0.75).abs() < 1e-9);

        stats.allocated_slots = Some(10);
        assert_eq!(stats.hole_rate(), 0.0);
    }
}
