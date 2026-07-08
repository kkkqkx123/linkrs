//! Statement-level planner
//!
//! A planner implementation that includes all statements for graph databases
//! All statement types supported by Cypher and NGQL are available.
//!
//! ## Architecture Description
//!
//! Adopts a three-layer architecture design:
//! `Planner trait`: The basic interface for planners.
//! `StatementPlanner` trait: A statement-level planner that processes entire statements.
//! `ClausePlanner` trait: A clause-level planner that processes individual clauses.

// Sub-modules organized by function
pub mod clauses;
pub mod ddl;
pub mod dml;
pub mod dql;
pub mod paths;
pub mod seeks;

// Core traits and special planners
pub mod match_statement_planner;
pub mod statement_planner;

// Re-export core traits
pub use statement_planner::{ClausePlanner, StatementPlanner};

// Re-export commonly used types
pub use match_statement_planner::{MatchPlannerConfig, MatchStatementPlanner};
