//! Graph Traversal Executor Module
//!
//! Include all executors related to graph traversal, including:
//!   - Step-by-step expansion
//!   - ExpandAll (full path extension)
//!   - Complete traversal
//!   - ShortestPath
//!   - AllPaths – Added
//!   - MultiShortestPath – Added
//!   - Subgraph extraction

// Algorithm module – Decouples the implementation of algorithms from the execution process
pub mod algorithms;
pub mod all_paths;
pub mod expand;
pub mod expand_all;
pub mod factory;
pub mod impls;
pub mod shortest_path;
pub mod tests;
pub mod traits;
pub mod traversal_utils;
pub mod traverse;

// Re-export the main types
pub use all_paths::AllPathsExecutor;
pub use expand::ExpandExecutor;
pub use expand_all::{ExpandAllExecutor, ExpandAllExecutorParams};
pub use shortest_path::ShortestPathExecutor;
pub use traverse::TraverseExecutor;

// Export Algorithm Module
pub use algorithms::{
    AStar, AlgorithmContext, AlgorithmStats, BidirectionalBFS, Dijkstra, PathFindingAlgorithm,
    ShortestPathAlgorithm, ShortestPathAlgorithmType, TraversalAlgorithm,
};

// Exporting common features and factory information
pub use factory::GraphTraversalExecutorFactory;
pub use traits::GraphTraversalExecutor;
