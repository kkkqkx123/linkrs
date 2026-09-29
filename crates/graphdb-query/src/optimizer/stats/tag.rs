//! Tag statistics module
//!
//! Provide tag-level statistical information for use in querying the estimates made by the optimization engine.

/// Tag statistics information
#[derive(Debug, Clone)]
pub struct TagStatistics {
    /// Tag name
    pub tag_name: String,
    /// Number of vertices
    pub vertex_count: u64,
    /// Allocated slots including deleted-but-unreclaimed holes, when the
    /// storage engine exposes a table cardinality snapshot. `None` means
    /// unknown and costs fall back to the live count (zero holes).
    pub allocated_slots: Option<u64>,
    /// Average Outdegree (Key Metric: Impact on the Cost of Traversal)
    pub avg_out_degree: f64,
    /// Average Indegree
    pub avg_in_degree: f64,
    /// Space this statistics was collected for (`None` = default).
    pub space: Option<String>,
    /// Schema version at collection time (used for staleness checks).
    pub schema_version: Option<u64>,
}

impl TagStatistics {
    /// Create new tag statistics information.
    pub fn new(tag_name: String) -> Self {
        Self {
            tag_name,
            vertex_count: 0,
            allocated_slots: None,
            avg_out_degree: 0.0,
            avg_in_degree: 0.0,
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

    /// Hole rate `1 - live / allocated`, clamped to `[0, 1)`.
    ///
    /// Unknown or inconsistent snapshots (allocated below the live count,
    /// empty tables) report zero so costing falls back to live rows. The
    /// snapshot is a shard-inconsistent sizing read, never a census.
    pub fn hole_rate(&self) -> f64 {
        match self.allocated_slots {
            Some(allocated)
                if allocated > 0 && allocated > self.vertex_count && self.vertex_count > 0 =>
            {
                1.0 - (self.vertex_count as f64 / allocated as f64)
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
}

impl Default for TagStatistics {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hole_rate_clamps_unknown_and_inconsistent() {
        let mut stats = TagStatistics::new("person".to_string());
        assert_eq!(stats.hole_rate(), 0.0);

        stats.vertex_count = 1000;
        stats.allocated_slots = Some(2000);
        assert!((stats.hole_rate() - 0.5).abs() < 1e-9);

        stats.allocated_slots = Some(10);
        assert_eq!(stats.hole_rate(), 0.0);

        stats.vertex_count = 0;
        stats.allocated_slots = Some(2000);
        assert_eq!(stats.hole_rate(), 0.0);
    }

    #[test]
    fn apply_table_snapshot_keeps_unknown_on_empty() {
        let mut stats = TagStatistics::new("person".to_string());
        stats.apply_table_snapshot(None);
        assert_eq!(stats.allocated_slots, None);
    }
}
