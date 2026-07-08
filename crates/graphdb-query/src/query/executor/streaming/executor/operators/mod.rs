//! Streaming operator implementations
//!
//! Organized by operator type:
//! - sources: ScanVertices, ScanEdges
//! - single_input: Filter, Project, Limit, Distinct
//! - stateful: Aggregate, Sort, GroupBy, WindowFunction
//! - binary: HashJoin, NestedLoopJoin
//! - set_ops: Union, UnionAll, Intersect, Except

pub mod binary;
pub mod set_ops;
pub mod single_input;
pub mod sources;
pub mod stateful;

pub use binary::{close_nestedloopjoin, close_hashjoin, next_hashjoin, next_nestedloopjoin, open_nestedloopjoin, open_hashjoin, stop_hashjoin, stop_nestedloopjoin};
pub use set_ops::{close_except, close_intersect, close_union, close_unionall, next_except, next_intersect, next_union, next_unionall, open_except, open_intersect, open_union, open_unionall, stop_except, stop_intersect, stop_union, stop_unionall};
pub use single_input::{close_distinct, close_filter, close_limit, close_project, next_distinct, next_filter, next_limit, next_project, open_distinct, open_filter, open_limit, open_project, stop_distinct, stop_filter, stop_limit, stop_project};
pub use sources::{close_scanedges, close_scanvertices, next_scanedges, next_scanvertices, open_scanedges, open_scanvertices, stop_scanedges, stop_scanvertices};
pub use stateful::{close_aggregate, close_groupby, close_sort, close_windowfunction, next_aggregate, next_groupby, next_sort, next_windowfunction, open_aggregate, open_groupby, open_sort, open_windowfunction, stop_aggregate, stop_groupby, stop_sort, stop_windowfunction};
