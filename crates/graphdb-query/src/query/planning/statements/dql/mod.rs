//! Data Query Language (DQL) Statement Planners
//!
//! This module contains **statement-level** planners for data query operations.
//! Each planner implements the `Planner` trait and is registered in `PlannerEnum`.
//!
//! ## Responsibility Boundary
//!
//! **Statement planners** (this module) handle standalone DQL statements.
//! They implement `Planner` and generate a complete `SubPlan` from scratch.
//! Example: `ReturnPlanner` handles a standalone `RETURN 1 AS x` statement.
//!
//! **Clause planners** (`clauses/` module) handle individual clauses within compound statements.
//! They implement `ClausePlanner` and receive an input `SubPlan` to build upon.
//! Example: `WhereClausePlanner` handles the WHERE clause inside a MATCH statement.
//!
//! When a clause appears both as a standalone statement and as a clause within
//! a compound statement, both modules may have a planner for it (e.g., `ReturnPlanner`
//! in this module vs `ReturnClausePlanner` in `clauses/`). The statement planner
//! handles the standalone case, while the clause planner is used by compound
//! statement planners like `MatchStatementPlanner`.
//!
//! ## Supported Operations
//!
//! - FETCH EDGES: Fetch edges by source/destination/rank
//! - FETCH VERTICES: Fetch vertices by ID
//! - GO: Nebula-style traversal queries
//! - GROUP BY: Aggregation queries
//! - LOOKUP: Index-based vertex/edge lookup
//! - PATH: Path finding queries (shortest path, all paths)
//! - RETURN: Return results (standalone statement)
//! - SET OPERATION: Union, Intersect, Minus operations
//! - SUBGRAPH: Subgraph expansion queries
//! - UNWIND: Expand list into multiple rows
//! - WITH: Pipe results between query parts
//! - YIELD: Yield results (standalone statement)
//!
//! ## Pipe Variable Support
//!
//! The `pipe_variable_resolver` module handles resolution of pipe variable references (`$-`)
//! in chained queries, enabling complex query pipelines.
//!
//! ## Composite Index Optimization
//!
//! The `composite_index_analyzer` module provides optimal index selection for LOOKUP queries
//! with multiple conditions, supporting prefix matching and range scans.

pub mod composite_index_analyzer;
pub mod explain_planner;
pub mod fetch_edges_planner;
pub mod fetch_vertices_planner;
pub mod go_planner;
pub mod group_by_planner;
pub mod lookup_planner;
pub mod path_planner;
pub mod pipe_planner;
pub mod pipe_variable_resolver;
pub mod return_planner;
pub mod set_operation_planner;
pub mod subgraph_planner;
pub mod unwind_planner;
pub mod with_planner;
pub mod yield_planner;

pub use composite_index_analyzer::{
    ColumnStats, CompositeIndexAnalyzer, CompositeIndexSelection, IndexSelectionResult, MatchType,
    PredicateInfo, PredicateOp, SingleColumnSelection,
};
pub use pipe_variable_resolver::{
    ColumnDataType, ColumnSchema, FromClausePlan, ParsedPipeVariable, PipeVariableResolver,
    ResolverError, VariableInfo, VariableSchema,
};
