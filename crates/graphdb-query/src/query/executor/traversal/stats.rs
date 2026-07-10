#[derive(Debug, Default, Clone)]
pub struct TraversalStats {
    pub vertices_visited: u64,
    pub edges_scanned: u64,
    pub paths_emitted: u64,
    pub max_depth_reached: u32,
    pub max_frontier_size: usize,
    pub storage_calls: u64,
}

impl TraversalStats {
    pub fn record_vertex_visit(&mut self) {
        self.vertices_visited += 1;
    }

    pub fn record_edge_scan(&mut self, count: usize) {
        self.edges_scanned += count as u64;
    }

    pub fn record_path_emitted(&mut self) {
        self.paths_emitted += 1;
    }

    pub fn record_storage_call(&mut self) {
        self.storage_calls += 1;
    }

    pub fn update_depth(&mut self, depth: u32) {
        if depth > self.max_depth_reached {
            self.max_depth_reached = depth;
        }
    }

    pub fn update_frontier(&mut self, size: usize) {
        if size > self.max_frontier_size {
            self.max_frontier_size = size;
        }
    }
}
