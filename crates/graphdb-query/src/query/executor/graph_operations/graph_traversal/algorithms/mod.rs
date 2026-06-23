//! Graph Algorithm Module
//!
//! Implementations of various graph traversal and pathfinding algorithms
//!
//! # List of Algorithms
//! *a_star*: An A* heuristic search algorithm
//! `bidirectional_bfs`: Algorithm for finding the shortest path using Bidirectional Breadth-First Search (BFS)
//! `bfs_shortest`: The executor for finding the shortest path using Breadth-First Search (BFS) algorithm.
//! Dijkstra: The Dijkstra algorithm for finding the shortest path
//! `multi_shortest_path`: An algorithm for finding the shortest paths from multiple sources to a single destination.
//! `subgraphExecutor`: The executor responsible for executing subgraph queries.

pub mod a_star;
pub mod bfs_shortest;
pub mod bidirectional_bfs;
pub mod dijkstra;
pub mod multi_shortest_path;
pub mod subgraph_executor;
pub mod traits;
pub mod types;

// Reexport the algorithm type
pub use a_star::AStar;
pub use bfs_shortest::BFSShortestExecutor;
pub use bidirectional_bfs::BidirectionalBFS;
pub use dijkstra::Dijkstra;
pub use multi_shortest_path::MultiShortestPathExecutor;
pub use subgraph_executor::{SubgraphConfig, SubgraphExecutor, SubgraphResult};
pub use traits::{
    AlgorithmContext, PathFindingAlgorithm, ShortestPathAlgorithm, TraversalAlgorithm,
};
pub use types::{
    cleanup_termination_map, combine_npaths, create_termination_map, has_duplicate_edges,
    is_termination_complete, mark_path_found, AlgorithmStats, BidirectionalBFSState, DistanceNode,
    EdgeWeightConfig, HeuristicFunction, Interims, MultiPathRequest, SelfLoopDedup,
    ShortestPathAlgorithmType, TerminationMap,
};
