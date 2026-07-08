//! Clause-Level Planner Module
//!
//! This module contains clause-level planners that implement the `ClausePlanner` trait.
//! Each planner handles a specific clause within a compound statement (e.g., MATCH).
//!
//! ## Responsibility Boundary
//!
//! **Clause planners** (this module) handle individual clauses within compound statements.
//! They implement `ClausePlanner` and receive an input `SubPlan` to build upon.
//! Example: `WhereClausePlanner` handles the WHERE clause inside a MATCH statement.
//!
//! **Statement planners** (`dql/` module) handle standalone DQL statements.
//! They implement the `Planner` trait and generate a complete `SubPlan` from scratch.
//! Example: `ReturnPlanner` handles a standalone `RETURN 1 AS x` statement.
//!
//! The key difference: clause planners are composable building blocks used by
//! statement planners (like `MatchStatementPlanner`), while statement planners
//! are top-level planners registered in `PlannerEnum`.

pub mod order_by_planner;
pub mod pagination_planner;
pub mod return_clause_planner;
pub mod unwind_planner;
pub mod where_clause_planner;
pub mod with_clause_planner;
pub mod yield_planner;

pub use order_by_planner::OrderByClausePlanner;
pub use pagination_planner::PaginationPlanner;
pub use return_clause_planner::ReturnClausePlanner;
pub use where_clause_planner::WhereClausePlanner;
pub use with_clause_planner::WithClausePlanner;
