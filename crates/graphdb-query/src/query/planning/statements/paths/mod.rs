//! Path Planner Module
//!
//! The path planner and the shortest path planner used in the MATCH query.
//!
//! ## Modules
//!
//! - `match_path_planner`: General path pattern planning for MATCH queries
//! - `shortest_path_planner`: BFS-based shortest path planning
//! - `variable_length_path_planner`: Optimized planning for variable-length paths `[:TYPE*min..max]`

pub mod match_path_planner;
pub mod shortest_path_planner;
pub mod variable_length_path_planner;

pub use match_path_planner::{
    EdgePattern, EdgeTraversal, EndCondition, MatchPathPlanner, PathPattern, PathPatternKind,
    PathPlan, StartVidFinder, VariableLengthPathPattern,
};
pub use shortest_path_planner::{
    BfsConfig, ShortestPath, ShortestPathPlan, ShortestPathPlanner, ShortestPathResult,
    StartVidSource,
};
pub use variable_length_path_planner::{
    PathExpansionStats, PathPruner, PruningConfig, PruningStrategy, VLPConfig, VLPStrategy,
    VariableLengthPathPlan, VariableLengthPathPlanner, VariableLengthPathSpec,
};
