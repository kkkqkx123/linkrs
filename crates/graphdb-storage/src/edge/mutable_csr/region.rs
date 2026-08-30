/// Per-region metadata for incremental freeze decision.
#[derive(Debug, Clone, PartialEq)]
pub struct MutableCsrRegion {
    pub region_id: u32,
    pub vertex_start: u32,
    pub vertex_end: u32,
    pub edge_count: u32,
    pub deleted_count: u32,
    pub capacity: u32,
    pub density: f32,
}

impl MutableCsrRegion {
    pub fn deletion_ratio(&self) -> f64 {
        if self.edge_count == 0 {
            0.0
        } else {
            self.deleted_count as f64 / self.edge_count as f64
        }
    }

    pub fn is_high_density(&self, threshold: f32) -> bool {
        self.density >= threshold
    }
}
