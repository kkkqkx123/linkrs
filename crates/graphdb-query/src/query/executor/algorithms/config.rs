use crate::core::types::VertexId;
use crate::core::types::EdgeDirection;

/// Multiple Starting Point Shortest Path Configuration
pub struct MultiShortestPathConfig {
    pub start_vids: Vec<VertexId>,
    pub direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_steps: usize,
    pub space_name: String,
}
