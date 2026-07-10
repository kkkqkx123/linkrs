//! Streaming operator implementations
//!
//! Organized by operator type:
//! - access: Start, GetVertices, GetEdges, GetNeighbors, IndexScan, EdgeIndexScan, Argument, Sample
//! - sources: ScanVertices, ScanEdges
//! - single_input: Filter, Project, Limit, Distinct
//! - stateful: Aggregate, Sort, GroupBy, WindowFunction
//! - binary: HashJoin, NestedLoopJoin
//! - set_ops: Union, UnionAll, Intersect, Except
//! - relational: TopN, Dedup, Assign, Materialize, Remove, DataCollect, Unwind, Apply, PatternApply, RollUpApply, Minus, Window
//! - data_modification: InsertVertices, InsertEdges, UpdateVertices, UpdateEdges, DeleteVertices, DeleteEdges, PipeDeleteVertices, PipeDeleteEdges
//! - analyze: Analyze (stats collection)
//! - migrate: Migrate (schema/data migration)
//! - graph_traversal: Expand, ExpandAll, Traverse, TraverseAll, AppendVertices, BiExpand, BiTraverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath

pub mod access;
pub mod binary;
pub mod control_flow;
pub mod data_modification;
pub mod graph_traversal;
pub mod management;
pub mod relational;
pub mod search;
pub mod set_ops;
pub mod single_input;
pub mod sources;
pub mod stateful;

pub use access::{
    close_argument, close_edgeindexscan, close_getedges, close_getneighbors, close_getprop,
    close_getvertices, close_indexscan, close_lookupindex, close_sample, close_start,
    next_argument, next_edgeindexscan, next_getedges, next_getneighbors, next_getprop,
    next_getvertices, next_indexscan, next_lookupindex, next_sample, next_start, open_argument,
    open_edgeindexscan, open_getedges, open_getneighbors, open_getprop, open_getvertices,
    open_indexscan, open_lookupindex, open_sample, open_start, stop_argument, stop_edgeindexscan,
    stop_getedges, stop_getneighbors, stop_getprop, stop_getvertices, stop_indexscan,
    stop_lookupindex, stop_sample, stop_start,
};
pub use binary::{
    close_crossjoin, close_fullouterjoin, close_hashjoin, close_innerjoin, close_leftjoin,
    close_nestedloopjoin, close_rightjoin, close_semijoin, next_crossjoin, next_fullouterjoin,
    next_hashjoin, next_innerjoin, next_leftjoin, next_nestedloopjoin, next_rightjoin,
    next_semijoin, open_crossjoin, open_fullouterjoin, open_hashjoin, open_innerjoin,
    open_leftjoin, open_nestedloopjoin, open_rightjoin, open_semijoin, stop_crossjoin,
    stop_fullouterjoin, stop_hashjoin, stop_innerjoin, stop_leftjoin, stop_nestedloopjoin,
    stop_rightjoin, stop_semijoin,
};
pub use control_flow::{
    close_begin_transaction, close_commit, close_loop, close_passthrough, close_rollback,
    close_select, close_show_stats, next_begin_transaction, next_commit, next_loop,
    next_passthrough, next_rollback, next_select, next_show_stats, open_begin_transaction,
    open_commit, open_loop, open_passthrough, open_rollback, open_select, open_show_stats,
    stop_begin_transaction, stop_commit, stop_loop, stop_passthrough, stop_rollback, stop_select,
    stop_show_stats,
};
pub use data_modification::{
    close_deleteedges, close_deletevertices, close_insertedges, close_insertvertices,
    close_pipedeleteedges, close_pipedeletevertices, close_updateedges, close_updatevertices,
    next_deleteedges, next_deletevertices, next_insertedges, next_insertvertices,
    next_pipedeleteedges, next_pipedeletevertices, next_updateedges, next_updatevertices,
    open_deleteedges, open_deletevertices, open_insertedges, open_insertvertices,
    open_pipedeleteedges, open_pipedeletevertices, open_updateedges, open_updatevertices,
    stop_deleteedges, stop_deletevertices, stop_insertedges, stop_insertvertices,
    stop_pipedeleteedges, stop_pipedeletevertices, stop_updateedges, stop_updatevertices,
};
pub use graph_traversal::{
    close_allpaths, close_appendvertices, close_bfsshortest, close_biexpand, close_bitraverse,
    close_expand, close_expandall, close_multishortestpath, close_shortestpath, close_traverse,
    close_traverseall, next_allpaths, next_appendvertices, next_bfsshortest, next_biexpand,
    next_bitraverse, next_expand, next_expandall, next_multishortestpath, next_shortestpath,
    next_traverse, next_traverseall, open_allpaths, open_appendvertices, open_bfsshortest,
    open_biexpand, open_bitraverse, open_expand, open_expandall, open_multishortestpath,
    open_shortestpath, open_traverse, open_traverseall, stop_allpaths, stop_appendvertices,
    stop_bfsshortest, stop_biexpand, stop_bitraverse, stop_expand, stop_expandall,
    stop_multishortestpath, stop_shortestpath, stop_traverse, stop_traverseall,
};
pub use management::{
    close_analyze, close_edge_manage, close_fulltext_manage, close_index_manage, close_migrate,
    close_space_manage, close_tag_manage, close_user_manage, close_vector_manage, next_analyze,
    next_edge_manage, next_fulltext_manage, next_index_manage, next_migrate, next_space_manage,
    next_tag_manage, next_user_manage, next_vector_manage, open_analyze, open_edge_manage,
    open_fulltext_manage, open_index_manage, open_migrate, open_space_manage, open_tag_manage,
    open_user_manage, open_vector_manage, stop_analyze, stop_edge_manage, stop_fulltext_manage,
    stop_index_manage, stop_migrate, stop_space_manage, stop_tag_manage, stop_user_manage,
    stop_vector_manage,
};
pub use relational::{
    close_apply, close_assign, close_datacollect, close_dedup, close_materialize, close_minus,
    close_patternapply, close_remove, close_rolluapply, close_topn, close_unwind, close_window,
    next_apply, next_assign, next_datacollect, next_dedup, next_materialize, next_minus,
    next_patternapply, next_remove, next_rolluapply, next_topn, next_unwind, next_window,
    open_apply, open_assign, open_datacollect, open_dedup, open_materialize, open_minus,
    open_patternapply, open_remove, open_rolluapply, open_topn, open_unwind, open_window,
    stop_apply, stop_assign, stop_datacollect, stop_dedup, stop_materialize, stop_minus,
    stop_patternapply, stop_remove, stop_rolluapply, stop_topn, stop_unwind, stop_window,
};
pub use search::{
    close_fulltext_lookup, close_fulltext_search, close_match_fulltext, close_vector_lookup,
    close_vector_search, next_fulltext_lookup, next_fulltext_search, next_match_fulltext,
    next_vector_lookup, next_vector_search, open_fulltext_lookup, open_fulltext_search,
    open_match_fulltext, open_vector_lookup, open_vector_search, stop_fulltext_lookup,
    stop_fulltext_search, stop_match_fulltext, stop_vector_lookup, stop_vector_search,
};
pub use set_ops::{
    close_except, close_intersect, close_union, close_unionall, next_except, next_intersect,
    next_union, next_unionall, open_except, open_intersect, open_union, open_unionall, stop_except,
    stop_intersect, stop_union, stop_unionall,
};
pub use single_input::{
    close_distinct, close_filter, close_limit, close_project, next_distinct, next_filter,
    next_limit, next_project, open_distinct, open_filter, open_limit, open_project, stop_distinct,
    stop_filter, stop_limit, stop_project,
};
pub use sources::{
    close_scanedges, close_scanvertices, next_scanedges, next_scanvertices, open_scanedges,
    open_scanvertices, stop_scanedges, stop_scanvertices,
};
pub use stateful::{
    close_aggregate, close_groupby, close_sort, close_windowfunction, next_aggregate, next_groupby,
    next_sort, next_windowfunction, open_aggregate, open_groupby, open_sort, open_windowfunction,
    stop_aggregate, stop_groupby, stop_sort, stop_windowfunction,
};
