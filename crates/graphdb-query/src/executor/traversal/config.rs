use graphdb_core::EdgeDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalKind {
    Expand,
    Traverse,
    ShortestPath,
    AllPaths,
    Subgraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalOrder {
    Bfs,
    Dfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitedPolicy {
    Global,
    PerSeed,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPolicy {
    VertexOnly,
    EdgeAndVertex,
    FullPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitPolicy {
    EveryDepth,
    LeafOnly,
    DestinationOnly,
}

#[derive(Debug, Clone)]
pub struct TraversalConfig {
    pub kind: TraversalKind,
    pub direction: EdgeDirection,
    pub edge_types: Vec<String>,
    pub min_depth: u32,
    pub max_depth: u32,
    pub limit: usize,
    pub order: TraversalOrder,
    pub visited_policy: VisitedPolicy,
    pub path_policy: PathPolicy,
    pub emit_policy: EmitPolicy,
    pub space_name: String,
}

impl TraversalConfig {
    pub fn expand(space_name: String, direction: EdgeDirection, edge_types: Vec<String>) -> Self {
        Self {
            kind: TraversalKind::Expand,
            direction,
            edge_types,
            min_depth: 1,
            max_depth: 1,
            limit: usize::MAX,
            order: TraversalOrder::Bfs,
            visited_policy: VisitedPolicy::None,
            path_policy: PathPolicy::EdgeAndVertex,
            emit_policy: EmitPolicy::EveryDepth,
            space_name,
        }
    }

    pub fn traverse(
        space_name: String,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        edge_types: Vec<String>,
    ) -> Self {
        Self {
            kind: TraversalKind::Traverse,
            direction,
            edge_types,
            min_depth,
            max_depth,
            limit: usize::MAX,
            order: TraversalOrder::Dfs,
            visited_policy: VisitedPolicy::Global,
            path_policy: PathPolicy::VertexOnly,
            emit_policy: EmitPolicy::EveryDepth,
            space_name,
        }
    }
}
