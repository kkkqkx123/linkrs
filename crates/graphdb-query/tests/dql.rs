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

#[path = "dql/aggregation.rs"]
mod aggregation;
mod common;
#[path = "dql/fetch.rs"]
mod fetch;
#[path = "dql/find_path.rs"]
mod find_path;
#[path = "dql/go.rs"]
mod go;
#[path = "dql/lookup.rs"]
mod lookup;
#[path = "dql/match_query.rs"]
mod match_query;
#[path = "dql/optimizer.rs"]
mod optimizer;
#[path = "dql/set_operations.rs"]
mod set_operations;
#[path = "dql/subgraph.rs"]
mod subgraph;
#[path = "dql/subquery.rs"]
mod subquery;
