//! Data Query Language (DQL) Integration Tests
//!
//! Test coverage:
//! - GO - Graph traversal
//! - MATCH - Pattern matching
//! - FETCH - Property fetching
//! - LOOKUP - Index-based lookup
//! - Aggregation - GROUP BY, ORDER BY, LIMIT
//! - Subquery - WITH, UNWIND
//! - FIND PATH - Path finding
//! - SUBGRAPH - Subgraph retrieval
//! - Set Operations - UNION, INTERSECT, MINUS
//! - Optimizer - Query plan optimization tests

mod aggregation;
mod common;
mod fetch;
mod find_path;
mod go;
mod lookup;
mod match_query;
mod optimizer;
mod set_operations;
mod subgraph;
mod subquery;
