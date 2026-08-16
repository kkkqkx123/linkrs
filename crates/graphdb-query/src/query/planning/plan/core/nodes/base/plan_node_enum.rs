//! Definition of the PlanNode enumeration
//!
//! This document defines the PlanNodeEnum enumeration, which includes all possible types of planning nodes.
//! Use macros to generate template code in order to reduce repetition.
//!
//! # Refactoring: Management Node Parameterization
//! Management nodes (DDL/DCL) have been grouped into category-based sub-enums
//! to reduce the total variant count from 90+ to ~50. Each management category
//! (Space, Tag, Edge, Index, User, Fulltext, Vector) is now a single variant
//! that wraps its corresponding sub-enum.

use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::data_modification::{
    DeleteEdgesNode, DeleteIndexNode, DeleteTagsNode, DeleteVerticesNode, InsertEdgesNode,
    InsertVerticesNode, PipeDeleteEdgesNode, PipeDeleteVerticesNode, UpdateEdgesNode, UpdateNode,
    UpdateVerticesNode,
};
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, FulltextManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
    UserManageNode, VectorManageNode,
};
use crate::query::planning::plan::core::nodes::management::stats_nodes::ShowStatsNode;
use crate::query::planning::plan::core::nodes::management::system_nodes::{
    ShowConfigsNode, ShowQueriesNode, ShowSessionsNode,
};
use crate::query::planning::plan::core::nodes::search::fulltext::data_access::{
    FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
};
#[cfg(feature = "qdrant")]
use crate::query::planning::plan::core::nodes::search::vector::data_access::{
    VectorLookupNode, VectorMatchNode, VectorSearchNode,
};

// Import and re-export all specific node types.
pub use crate::query::planning::plan::core::nodes::access::graph_scan_node::{
    GetEdgesNode, GetNeighborsNode, GetVerticesNode, ScanEdgesNode, ScanVerticesNode,
};
pub use crate::query::planning::plan::core::nodes::access::index_scan::{
    IndexLimit, IndexScanNode, OrderByItem, ScanType,
};
pub use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::{
    ArgumentNode, BeginTransactionNode, CommitNode, LoopNode, PassThroughNode,
    ReleaseSavepointNode, RollbackNode, SavepointNode, SelectNode,
};
pub use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
pub use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
pub use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    ApplyNode, AssignNode, CorrelatedApplyNode, DataCollectNode, DedupNode, MaterializeNode,
    PatternApplyNode, RemoveNode, RollUpApplyNode, UnionNode, UnwindNode,
};
pub use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::{
    IntersectNode, MinusNode,
};
pub use crate::query::planning::plan::core::nodes::graph_operations::window_node::{
    WindowFunctionSpec, WindowNode,
};
pub use crate::query::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, FullOuterJoinNode, InnerJoinNode, LeftJoinNode, RightJoinNode, SemiJoinNode,
};
pub use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
pub use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
pub use crate::query::planning::plan::core::nodes::operation::sample_node::SampleNode;
pub use crate::query::planning::plan::core::nodes::operation::sort_node::{
    LimitNode, SortNode, TopNNode,
};
pub use crate::query::planning::plan::core::nodes::traversal::path_algorithms::{
    AllPathsNode, BFSShortestNode, MultiShortestPathNode, ShortestPathNode,
};
pub use crate::query::planning::plan::core::nodes::traversal::traversal_node::{
    AppendVerticesNode, BiExpandNode, BiTraverseNode, ExpandAllNode, ExpandNode, TraverseNode,
};
// Re-export management sub-enums for external use
pub use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode as EdgeManageNodeEnum, FulltextManageNode as FulltextManageNodeEnum,
    IndexManageNode as IndexManageNodeEnum, SpaceManageNode as SpaceManageNodeEnum,
    TagManageNode as TagManageNodeEnum, UserManageNode as UserManageNodeEnum,
    VectorManageNode as VectorManageNodeEnum,
};
// Re-export individual management node types for backward compatibility
pub use crate::query::planning::plan::core::nodes::management::edge_nodes::{
    AlterEdgeNode, CreateEdgeNode, DescEdgeNode, DropEdgeNode, EdgeAlterInfo, EdgeManageInfo,
    ShowCreateEdgeNode, ShowEdgesNode,
};
pub use crate::query::planning::plan::core::nodes::management::index_nodes::{
    CreateEdgeIndexNode, CreateTagIndexNode, DescEdgeIndexNode, DescTagIndexNode,
    DropEdgeIndexNode, DropTagIndexNode, IndexManageInfo, RebuildEdgeIndexNode,
    RebuildTagIndexNode, ShowCreateIndexNode, ShowEdgeIndexesNode, ShowIndexesNode,
    ShowTagIndexesNode,
};
pub use crate::query::planning::plan::core::nodes::management::space_nodes::{
    AlterSpaceNode, ClearSpaceNode, CreateSpaceNode, DescSpaceNode, DropSpaceNode,
    ShowCreateSpaceNode, ShowSpacesNode, SpaceAlterOption, SpaceManageInfo, SwitchSpaceNode,
};
pub use crate::query::planning::plan::core::nodes::management::stats_nodes::{
    ShowStatsNode as ShowStatsNodeType, ShowStatsType,
};
pub use crate::query::planning::plan::core::nodes::management::system_nodes::{
    ShowConfigsNode as ShowConfigsNodeType, ShowQueriesNode as ShowQueriesNodeType,
    ShowSessionsNode as ShowSessionsNodeType,
};
pub use crate::query::planning::plan::core::nodes::management::tag_nodes::{
    AlterTagNode, CreateTagNode, DescTagNode, DropTagNode, ShowCreateTagNode, ShowTagsNode,
    TagAlterInfo, TagManageInfo,
};
pub use crate::query::planning::plan::core::nodes::management::user_nodes::{
    AlterUserNode, ChangePasswordNode, CreateUserNode, DropUserNode, GrantRoleNode, RevokeRoleNode,
    ShowRolesNode, ShowUsersNode,
};
pub use crate::query::planning::plan::core::nodes::search::fulltext::management::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, ShowFulltextIndexNode,
};
pub use crate::query::planning::plan::core::nodes::search::vector::management::{
    CreateVectorIndexNode, DropVectorIndexNode,
};

/// The PlanNode enumeration includes all possible node types.
///
/// This enumeration avoids the performance overhead associated with dynamic distribution.
///
/// # Management Node Parameterization
/// Management nodes are grouped into category-based sub-enums:
/// - `SpaceManage(SpaceManageNode)` - Space DDL operations
/// - `TagManage(TagManageNode)` - Tag DDL operations
/// - `EdgeManage(EdgeManageNode)` - Edge DDL operations
/// - `IndexManage(IndexManageNode)` - Index DDL operations
/// - `UserManage(UserManageNode)` - User DDL operations
/// - `FulltextManage(FulltextManageNode)` - Fulltext index DDL operations
/// - `VectorManage(VectorManageNode)` - Vector index DDL operations
#[derive(Debug, Clone)]
pub enum PlanNodeEnum {
    // Access Node
    Start(StartNode),
    GetVertices(GetVerticesNode),
    GetEdges(GetEdgesNode),
    GetNeighbors(GetNeighborsNode),
    ScanVertices(ScanVerticesNode),
    ScanEdges(ScanEdgesNode),
    IndexScan(IndexScanNode),

    // Operation Node
    Project(ProjectNode),
    Filter(FilterNode),
    Sort(SortNode),
    Limit(LimitNode),
    TopN(TopNNode),
    Sample(SampleNode),
    Dedup(DedupNode),
    Aggregate(AggregateNode),
    Window(WindowNode),

    // ========== Connecting Nodes ==========
    InnerJoin(InnerJoinNode),
    LeftJoin(LeftJoinNode),
    RightJoin(RightJoinNode),
    CrossJoin(CrossJoinNode),
    FullOuterJoin(FullOuterJoinNode),
    SemiJoin(SemiJoinNode),

    // Traversal of nodes
    Expand(ExpandNode),
    ExpandAll(ExpandAllNode),
    Traverse(TraverseNode),
    AppendVertices(AppendVerticesNode),
    BiExpand(BiExpandNode),
    BiTraverse(BiTraverseNode),

    // ========== Control Flow Nodes ==========
    Argument(ArgumentNode),
    Loop(LoopNode),
    PassThrough(PassThroughNode),
    Select(SelectNode),

    // Transaction Control Nodes
    BeginTransaction(BeginTransactionNode),
    Commit(CommitNode),
    Rollback(RollbackNode),
    Savepoint(SavepointNode),
    ReleaseSavepoint(ReleaseSavepointNode),

    // ========== Data Processing Node ----------
    DataCollect(DataCollectNode),
    Remove(RemoveNode),
    PatternApply(PatternApplyNode),
    RollUpApply(RollUpApplyNode),
    CorrelatedApply(CorrelatedApplyNode),
    Union(UnionNode),
    Minus(MinusNode),
    Intersect(IntersectNode),
    Unwind(UnwindNode),
    Materialize(MaterializeNode),
    Assign(AssignNode),
    Apply(ApplyNode),

    // Algorithm Nodes
    MultiShortestPath(MultiShortestPathNode),
    BFSShortest(BFSShortestNode),
    AllPaths(AllPathsNode),
    ShortestPath(ShortestPathNode),

    // ========== Management Nodes (parameterized) ==========
    SpaceManage(SpaceManageNode),
    TagManage(TagManageNode),
    EdgeManage(EdgeManageNode),
    IndexManage(IndexManageNode),
    UserManage(UserManageNode),
    FulltextManage(FulltextManageNode),
    VectorManage(VectorManageNode),

    // Management Node – Data
    InsertVertices(InsertVerticesNode),
    InsertEdges(InsertEdgesNode),
    DeleteVertices(DeleteVerticesNode),
    DeleteEdges(DeleteEdgesNode),
    DeleteTags(DeleteTagsNode),
    DeleteIndex(DeleteIndexNode),
    PipeDeleteVertices(PipeDeleteVerticesNode),
    PipeDeleteEdges(PipeDeleteEdgesNode),
    Update(UpdateNode),
    UpdateVertices(UpdateVerticesNode),
    UpdateEdges(UpdateEdgesNode),

    // Statistics Nodes ============
    ShowStats(ShowStatsNode),

    // System Information Nodes ============
    ShowConfigs(ShowConfigsNode),
    ShowQueries(ShowQueriesNode),
    ShowSessions(ShowSessionsNode),

    // Full-text Search Nodes
    FulltextSearch(FulltextSearchNode),
    FulltextLookup(FulltextLookupNode),
    MatchFulltext(MatchFulltextNode),

    // Vector Search Nodes
    #[cfg(feature = "qdrant")]
    VectorSearch(VectorSearchNode),
    #[cfg(feature = "qdrant")]
    VectorLookup(VectorLookupNode),
    #[cfg(feature = "qdrant")]
    VectorMatch(VectorMatchNode),
}

impl Default for PlanNodeEnum {
    fn default() -> Self {
        PlanNodeEnum::Start(StartNode::new())
    }
}

// Use macros to generate the is_xxx method.
crate::define_enum_is_methods! {
    PlanNodeEnum,
    // Access node
    (Start, is_start),
    (GetVertices, is_get_vertices),
    (GetEdges, is_get_edges),
    (GetNeighbors, is_get_neighbors),
    (ScanVertices, is_scan_vertices),
    (ScanEdges, is_scan_edges),
    (IndexScan, is_index_scan),
    // Operation node
    (Project, is_project),
    (Filter, is_filter),
    (Sort, is_sort),
    (Limit, is_limit),
    (TopN, is_topn),
    (Sample, is_sample),
    (Dedup, is_dedup),
    (Aggregate, is_aggregate),
    (Window, is_window),
    // Connecting nodes
    (InnerJoin, is_inner_join),
    (LeftJoin, is_left_join),
    (RightJoin, is_right_join),
    (CrossJoin, is_cross_join),
    (FullOuterJoin, is_full_outer_join),
    (SemiJoin, is_semi_join),
    // Traverse the nodes
    (Expand, is_expand),
    (ExpandAll, is_expand_all),
    (Traverse, is_traverse),
    (AppendVertices, is_append_vertices),
    (BiExpand, is_bi_expand),
    (BiTraverse, is_bi_traverse),
    // Control flow nodes
    (Argument, is_argument),
    (Loop, is_loop),
    (PassThrough, is_pass_through),
    (Select, is_select),
    // Data processing node
    (DataCollect, is_data_collect),
    (Remove, is_remove),
    (PatternApply, is_pattern_apply),
    (RollUpApply, is_roll_up_apply),
    (CorrelatedApply, is_correlated_apply),
    (Union, is_union),
    (Minus, is_minus),
    (Intersect, is_intersect),
    (Unwind, is_unwind),
    (Materialize, is_materialize),
    (Assign, is_assign),
    (Apply, is_apply),
    // Algorithm node
    (MultiShortestPath, is_multi_shortest_path),
    (BFSShortest, is_bfs_shortest),
    (AllPaths, is_all_paths),
    (ShortestPath, is_shortest_path),
    // Management Node (parameterized)
    (SpaceManage, is_space_manage),
    (TagManage, is_tag_manage),
    (EdgeManage, is_edge_manage),
    (IndexManage, is_index_manage),
    (UserManage, is_user_manage),
    (FulltextManage, is_fulltext_manage),
    (VectorManage, is_vector_manage),
    // Management Node – Data
    (InsertVertices, is_insert_vertices),
    (InsertEdges, is_insert_edges),
    (DeleteVertices, is_delete_vertices),
    (DeleteEdges, is_delete_edges),
    (DeleteTags, is_delete_tags),
    (DeleteIndex, is_delete_index),
    (PipeDeleteVertices, is_pipe_delete_vertices),
    (PipeDeleteEdges, is_pipe_delete_edges),
    (Update, is_update),
    (UpdateVertices, is_update_vertices),
    (UpdateEdges, is_update_edges),
    // Statistical nodes
    (ShowStats, is_show_stats),
    (ShowConfigs, is_show_configs),
    (ShowQueries, is_show_queries),
    (ShowSessions, is_show_sessions),
    // Full-text Search Nodes
    (FulltextSearch, is_fulltext_search),
    (FulltextLookup, is_fulltext_lookup),
    (MatchFulltext, is_match_fulltext),
    // Vector Search Nodes
}

#[cfg(feature = "qdrant")]
crate::define_enum_is_methods! {
    PlanNodeEnum,
    (VectorSearch, is_vector_search),
    (VectorLookup, is_vector_lookup),
    (VectorMatch, is_vector_match),
}

// Use macros to generate the as_xxx method.
crate::define_enum_as_methods! {
    PlanNodeEnum,
    // Access node
    (Start, as_start, StartNode),
    (GetVertices, as_get_vertices, GetVerticesNode),
    (GetEdges, as_get_edges, GetEdgesNode),
    (GetNeighbors, as_get_neighbors, GetNeighborsNode),
    (ScanVertices, as_scan_vertices, ScanVerticesNode),
    (ScanEdges, as_scan_edges, ScanEdgesNode),
    (IndexScan, as_index_scan, IndexScanNode),
    // Operation node
    (Project, as_project, ProjectNode),
    (Filter, as_filter, FilterNode),
    (Sort, as_sort, SortNode),
    (Limit, as_limit, LimitNode),
    (TopN, as_topn, TopNNode),
    (Sample, as_sample, SampleNode),
    (Dedup, as_dedup, DedupNode),
    (Aggregate, as_aggregate, AggregateNode),
    (Window, as_window, WindowNode),
    // Connecting nodes
    (InnerJoin, as_inner_join, InnerJoinNode),
    (LeftJoin, as_left_join, LeftJoinNode),
    (CrossJoin, as_cross_join, CrossJoinNode),
    (FullOuterJoin, as_full_outer_join, FullOuterJoinNode),
    // Traverse the nodes
    (Expand, as_expand, ExpandNode),
    (ExpandAll, as_expand_all, ExpandAllNode),
    (Traverse, as_traverse, TraverseNode),
    (AppendVertices, as_append_vertices, AppendVerticesNode),
    // Control flow nodes
    (Argument, as_argument, ArgumentNode),
    (Loop, as_loop, LoopNode),
    (PassThrough, as_pass_through, PassThroughNode),
    (Select, as_select, SelectNode),
    // Transaction control nodes
    (BeginTransaction, as_begin_transaction, BeginTransactionNode),
    (Commit, as_commit, CommitNode),
    (Rollback, as_rollback, RollbackNode),
    (Savepoint, as_savepoint, SavepointNode),
    (ReleaseSavepoint, as_release_savepoint, ReleaseSavepointNode),
    // Data processing node
    (DataCollect, as_data_collect, DataCollectNode),
    (Remove, as_remove, RemoveNode),
    (PatternApply, as_pattern_apply, PatternApplyNode),
    (RollUpApply, as_roll_up_apply, RollUpApplyNode),
    (CorrelatedApply, as_correlated_apply, CorrelatedApplyNode),
    (Union, as_union, UnionNode),
    (Minus, as_minus, MinusNode),
    (Intersect, as_intersect, IntersectNode),
    (Unwind, as_unwind, UnwindNode),
    (Materialize, as_materialize, MaterializeNode),
    (Assign, as_assign, AssignNode),
    // Algorithm node
    (MultiShortestPath, as_multi_shortest_path, MultiShortestPathNode),
    (BFSShortest, as_bfs_shortest, BFSShortestNode),
    (AllPaths, as_all_paths, AllPathsNode),
    (ShortestPath, as_shortest_path, ShortestPathNode),
    // Management Node (parameterized)
    (SpaceManage, as_space_manage, SpaceManageNode),
    (TagManage, as_tag_manage, TagManageNode),
    (EdgeManage, as_edge_manage, EdgeManageNode),
    (IndexManage, as_index_manage, IndexManageNode),
    (UserManage, as_user_manage, UserManageNode),
    (FulltextManage, as_fulltext_manage, FulltextManageNode),
    (VectorManage, as_vector_manage, VectorManageNode),
    // Management Node – Data
    (InsertVertices, as_insert_vertices, InsertVerticesNode),
    (InsertEdges, as_insert_edges, InsertEdgesNode),
    (DeleteVertices, as_delete_vertices, DeleteVerticesNode),
    (DeleteEdges, as_delete_edges, DeleteEdgesNode),
    (DeleteTags, as_delete_tags, DeleteTagsNode),
    (DeleteIndex, as_delete_index, DeleteIndexNode),
    (PipeDeleteVertices, as_pipe_delete_vertices, PipeDeleteVerticesNode),
    (PipeDeleteEdges, as_pipe_delete_edges, PipeDeleteEdgesNode),
    (Update, as_update, UpdateNode),
    (UpdateVertices, as_update_vertices, UpdateVerticesNode),
    (UpdateEdges, as_update_edges, UpdateEdgesNode),
    // Statistical node
    (ShowStats, as_show_stats, ShowStatsNode),
    (ShowConfigs, as_show_configs, ShowConfigsNode),
    (ShowQueries, as_show_queries, ShowQueriesNode),
    (ShowSessions, as_show_sessions, ShowSessionsNode),
    // Full-text Search Nodes
    (FulltextSearch, as_fulltext_search, FulltextSearchNode),
    (FulltextLookup, as_fulltext_lookup, FulltextLookupNode),
    (MatchFulltext, as_match_fulltext, MatchFulltextNode),
    // Vector Search Nodes
}

#[cfg(feature = "qdrant")]
crate::define_enum_as_methods! {
    PlanNodeEnum,
    (VectorSearch, as_vector_search, VectorSearchNode),
    (VectorLookup, as_vector_lookup, VectorLookupNode),
    (VectorMatch, as_vector_match, VectorMatchNode),
}

// Use macros to generate the _xxx_mut method.
crate::define_enum_as_mut_methods! {
    PlanNodeEnum,
    // Access node
    (Start, as_start_mut, StartNode),
    (GetVertices, as_get_vertices_mut, GetVerticesNode),
    (GetEdges, as_get_edges_mut, GetEdgesNode),
    (GetNeighbors, as_get_neighbors_mut, GetNeighborsNode),
    (ScanVertices, as_scan_vertices_mut, ScanVerticesNode),
    (ScanEdges, as_scan_edges_mut, ScanEdgesNode),
    (IndexScan, as_index_scan_mut, IndexScanNode),
    // Operation node
    (Project, as_project_mut, ProjectNode),
    (Filter, as_filter_mut, FilterNode),
    (Sort, as_sort_mut, SortNode),
    (Limit, as_limit_mut, LimitNode),
    (TopN, as_topn_mut, TopNNode),
    (Sample, as_sample_mut, SampleNode),
    (Dedup, as_dedup_mut, DedupNode),
    (Aggregate, as_aggregate_mut, AggregateNode),
    (Window, as_window_mut, WindowNode),
    // Connecting nodes
    (InnerJoin, as_inner_join_mut, InnerJoinNode),
    (LeftJoin, as_left_join_mut, LeftJoinNode),
    (CrossJoin, as_cross_join_mut, CrossJoinNode),
    (FullOuterJoin, as_full_outer_join_mut, FullOuterJoinNode),
    // Traverse the nodes
    (Expand, as_expand_mut, ExpandNode),
    (ExpandAll, as_expand_all_mut, ExpandAllNode),
    (Traverse, as_traverse_mut, TraverseNode),
    (AppendVertices, as_append_vertices_mut, AppendVerticesNode),
    // Control flow nodes
    (Argument, as_argument_mut, ArgumentNode),
    (Loop, as_loop_mut, LoopNode),
    (PassThrough, as_pass_through_mut, PassThroughNode),
    (Select, as_select_mut, SelectNode),
    // Transaction control nodes
    (BeginTransaction, as_begin_transaction_mut, BeginTransactionNode),
    (Commit, as_commit_mut, CommitNode),
    (Rollback, as_rollback_mut, RollbackNode),
    (Savepoint, as_savepoint_mut, SavepointNode),
    (ReleaseSavepoint, as_release_savepoint_mut, ReleaseSavepointNode),
    // Data processing node
    (DataCollect, as_data_collect_mut, DataCollectNode),
    (Remove, as_remove_mut, RemoveNode),
    (PatternApply, as_pattern_apply_mut, PatternApplyNode),
    (RollUpApply, as_roll_up_apply_mut, RollUpApplyNode),
    (CorrelatedApply, as_correlated_apply_mut, CorrelatedApplyNode),
    (Union, as_union_mut, UnionNode),
    (Minus, as_minus_mut, MinusNode),
    (Intersect, as_intersect_mut, IntersectNode),
    (Unwind, as_unwind_mut, UnwindNode),
    (Materialize, as_materialize_mut, MaterializeNode),
    (Assign, as_assign_mut, AssignNode),
    // Algorithm node
    (MultiShortestPath, as_multi_shortest_path_mut, MultiShortestPathNode),
    (BFSShortest, as_bfs_shortest_mut, BFSShortestNode),
    (AllPaths, as_all_paths_mut, AllPathsNode),
    (ShortestPath, as_shortest_path_mut, ShortestPathNode),
    // Management Node (parameterized)
    (SpaceManage, as_space_manage_mut, SpaceManageNode),
    (TagManage, as_tag_manage_mut, TagManageNode),
    (EdgeManage, as_edge_manage_mut, EdgeManageNode),
    (IndexManage, as_index_manage_mut, IndexManageNode),
    (UserManage, as_user_manage_mut, UserManageNode),
    (FulltextManage, as_fulltext_manage_mut, FulltextManageNode),
    (VectorManage, as_vector_manage_mut, VectorManageNode),
    // Management Node – Data
    (InsertVertices, as_insert_vertices_mut, InsertVerticesNode),
    (InsertEdges, as_insert_edges_mut, InsertEdgesNode),
    (DeleteVertices, as_delete_vertices_mut, DeleteVerticesNode),
    (DeleteEdges, as_delete_edges_mut, DeleteEdgesNode),
    (DeleteTags, as_delete_tags_mut, DeleteTagsNode),
    (DeleteIndex, as_delete_index_mut, DeleteIndexNode),
    (PipeDeleteVertices, as_pipe_delete_vertices_mut, PipeDeleteVerticesNode),
    (PipeDeleteEdges, as_pipe_delete_edges_mut, PipeDeleteEdgesNode),
    (Update, as_update_mut, UpdateNode),
    (UpdateVertices, as_update_vertices_mut, UpdateVerticesNode),
    (UpdateEdges, as_update_edges_mut, UpdateEdgesNode),
    // Statistical node
    (ShowStats, as_show_stats_mut, ShowStatsNode),
    (ShowConfigs, as_show_configs_mut, ShowConfigsNode),
    (ShowQueries, as_show_queries_mut, ShowQueriesNode),
    (ShowSessions, as_show_sessions_mut, ShowSessionsNode),
    // Full-text Search Nodes
    (FulltextSearch, as_fulltext_search_mut, FulltextSearchNode),
    (FulltextLookup, as_fulltext_lookup_mut, FulltextLookupNode),
    (MatchFulltext, as_match_fulltext_mut, MatchFulltextNode),
    // Vector Search Nodes
}

#[cfg(feature = "qdrant")]
crate::define_enum_as_mut_methods! {
    PlanNodeEnum,
    (VectorSearch, as_vector_search_mut, VectorSearchNode),
    (VectorLookup, as_vector_lookup_mut, VectorLookupNode),
    (VectorMatch, as_vector_match_mut, VectorMatchNode),
}

// Use macros to generate the type_name method.
crate::define_enum_type_name! {
    PlanNodeEnum,
    // Access node
    (Start, "Start"),
    (GetVertices, "GetVertices"),
    (GetEdges, "GetEdges"),
    (GetNeighbors, "GetNeighbors"),
    (ScanVertices, "ScanVertices"),
    (ScanEdges, "ScanEdges"),
    (IndexScan, "IndexScan"),
    // Operation node
    (Project, "Project"),
    (Filter, "Filter"),
    (Sort, "Sort"),
    (Limit, "Limit"),
    (TopN, "TopN"),
    (Sample, "Sample"),
    (Dedup, "Dedup"),
    (Aggregate, "Aggregate"),
    (Window, "Window"),
    // Connecting nodes
    (InnerJoin, "InnerJoin"),
    (LeftJoin, "LeftJoin"),
    (RightJoin, "RightJoin"),
    (CrossJoin, "CrossJoin"),
    (FullOuterJoin, "FullOuterJoin"),
    (SemiJoin, "SemiJoin"),
    // Traverse the nodes
    (Expand, "Expand"),
    (ExpandAll, "ExpandAll"),
    (Traverse, "Traverse"),
    (AppendVertices, "AppendVertices"),
    (BiExpand, "BiExpand"),
    (BiTraverse, "BiTraverse"),
    // Control flow nodes
    (Argument, "Argument"),
    (Loop, "Loop"),
    (PassThrough, "PassThrough"),
    (Select, "Select"),
    // Transaction control nodes
    (BeginTransaction, "BeginTransaction"),
    (Commit, "Commit"),
    (Rollback, "Rollback"),
    (Savepoint, "Savepoint"),
    (ReleaseSavepoint, "ReleaseSavepoint"),
    // Data processing node
    (DataCollect, "DataCollect"),
    (Remove, "Remove"),
    (PatternApply, "PatternApply"),
    (RollUpApply, "RollUpApply"),
    (CorrelatedApply, "CorrelatedApply"),
    (Union, "Union"),
    (Minus, "Minus"),
    (Intersect, "Intersect"),
    (Unwind, "Unwind"),
    (Materialize, "Materialize"),
    (Assign, "Assign"),
    (Apply, "Apply"),
    // Algorithm node
    (MultiShortestPath, "MultiShortestPath"),
    (BFSShortest, "BFSShortest"),
    (AllPaths, "AllPaths"),
    (ShortestPath, "ShortestPath"),
    // Management Node (parameterized)
    (SpaceManage, "SpaceManage"),
    (TagManage, "TagManage"),
    (EdgeManage, "EdgeManage"),
    (IndexManage, "IndexManage"),
    (UserManage, "UserManage"),
    (FulltextManage, "FulltextManage"),
    (VectorManage, "VectorManage"),
    // Management Node – Data
    (InsertVertices, "InsertVertices"),
    (InsertEdges, "InsertEdges"),
    (DeleteVertices, "DeleteVertices"),
    (DeleteEdges, "DeleteEdges"),
    (DeleteTags, "DeleteTags"),
    (DeleteIndex, "DeleteIndex"),
    (PipeDeleteVertices, "PipeDeleteVertices"),
    (PipeDeleteEdges, "PipeDeleteEdges"),
    (Update, "Update"),
    (UpdateVertices, "UpdateVertices"),
    (UpdateEdges, "UpdateEdges"),
    // Statistical nodes
    (ShowStats, "ShowStats"),
    (ShowConfigs, "ShowConfigs"),
    (ShowQueries, "ShowQueries"),
    (ShowSessions, "ShowSessions"),
    // Full-text Search Nodes
    (FulltextSearch, "FulltextSearch"),
    (FulltextLookup, "FulltextLookup"),
    (MatchFulltext, "MatchFulltext"),
    // Vector Search Nodes
    #[cfg(feature = "qdrant")]
    (VectorSearch, "VectorSearch"),
    #[cfg(feature = "qdrant")]
    (VectorLookup, "VectorLookup"),
    #[cfg(feature = "qdrant")]
    (VectorMatch, "VectorMatch"),
}

// Use macros to generate the category method.
crate::define_enum_category! {
    PlanNodeEnum,
    // Access node
    (Start, PlanNodeCategory::Access),
    (GetVertices, PlanNodeCategory::Access),
    (GetEdges, PlanNodeCategory::Access),
    (GetNeighbors, PlanNodeCategory::Access),
    (ScanVertices, PlanNodeCategory::Access),
    (ScanEdges, PlanNodeCategory::Access),
    (IndexScan, PlanNodeCategory::Access),
    // Operation node
    (Project, PlanNodeCategory::Operation),
    (Filter, PlanNodeCategory::Operation),
    (Sort, PlanNodeCategory::Operation),
    (Limit, PlanNodeCategory::Operation),
    (TopN, PlanNodeCategory::Operation),
    (Sample, PlanNodeCategory::Operation),
    (Dedup, PlanNodeCategory::Operation),
    (Aggregate, PlanNodeCategory::Operation),
    (Window, PlanNodeCategory::Operation),
    // Connecting nodes
    (InnerJoin, PlanNodeCategory::Join),
    (LeftJoin, PlanNodeCategory::Join),
    (RightJoin, PlanNodeCategory::Join),
    (CrossJoin, PlanNodeCategory::Join),
    (FullOuterJoin, PlanNodeCategory::Join),
    (SemiJoin, PlanNodeCategory::Join),
    // Traverse the nodes
    (Expand, PlanNodeCategory::Traversal),
    (ExpandAll, PlanNodeCategory::Traversal),
    (Traverse, PlanNodeCategory::Traversal),
    (AppendVertices, PlanNodeCategory::Traversal),
    (BiExpand, PlanNodeCategory::Traversal),
    (BiTraverse, PlanNodeCategory::Traversal),
    // Control flow nodes
    (Argument, PlanNodeCategory::ControlFlow),
    (Loop, PlanNodeCategory::ControlFlow),
    (PassThrough, PlanNodeCategory::ControlFlow),
    (Select, PlanNodeCategory::ControlFlow),
    // Transaction control nodes
    (BeginTransaction, PlanNodeCategory::ControlFlow),
    (Commit, PlanNodeCategory::ControlFlow),
    (Rollback, PlanNodeCategory::ControlFlow),
    (Savepoint, PlanNodeCategory::ControlFlow),
    (ReleaseSavepoint, PlanNodeCategory::ControlFlow),
    // Data processing node
    (DataCollect, PlanNodeCategory::DataProcessing),
    (Remove, PlanNodeCategory::DataProcessing),
    (PatternApply, PlanNodeCategory::DataProcessing),
    (RollUpApply, PlanNodeCategory::DataProcessing),
    (CorrelatedApply, PlanNodeCategory::DataProcessing),
    (Union, PlanNodeCategory::DataProcessing),
    (Minus, PlanNodeCategory::DataProcessing),
    (Intersect, PlanNodeCategory::DataProcessing),
    (Unwind, PlanNodeCategory::DataProcessing),
    (Materialize, PlanNodeCategory::DataProcessing),
    (Assign, PlanNodeCategory::DataProcessing),
    (Apply, PlanNodeCategory::DataProcessing),
    // Algorithm node
    (MultiShortestPath, PlanNodeCategory::Algorithm),
    (BFSShortest, PlanNodeCategory::Algorithm),
    (AllPaths, PlanNodeCategory::Algorithm),
    (ShortestPath, PlanNodeCategory::Algorithm),
    // Management Node (parameterized)
    (SpaceManage, PlanNodeCategory::Management),
    (TagManage, PlanNodeCategory::Management),
    (EdgeManage, PlanNodeCategory::Management),
    (IndexManage, PlanNodeCategory::Management),
    (UserManage, PlanNodeCategory::Management),
    (FulltextManage, PlanNodeCategory::Management),
    (VectorManage, PlanNodeCategory::Management),
    // Management Node – Data
    (InsertVertices, PlanNodeCategory::Management),
    (InsertEdges, PlanNodeCategory::Management),
    (DeleteVertices, PlanNodeCategory::Management),
    (DeleteEdges, PlanNodeCategory::Management),
    (DeleteTags, PlanNodeCategory::Management),
    (DeleteIndex, PlanNodeCategory::Management),
    (PipeDeleteVertices, PlanNodeCategory::Management),
    (PipeDeleteEdges, PlanNodeCategory::Management),
    (Update, PlanNodeCategory::Management),
    (UpdateVertices, PlanNodeCategory::Management),
    (UpdateEdges, PlanNodeCategory::Management),
    // Statistical nodes
    (ShowStats, PlanNodeCategory::Management),
    (ShowConfigs, PlanNodeCategory::Management),
    (ShowQueries, PlanNodeCategory::Management),
    (ShowSessions, PlanNodeCategory::Management),
    // Full-text Search Nodes
    (FulltextSearch, PlanNodeCategory::DataAccess),
    (FulltextLookup, PlanNodeCategory::DataAccess),
    (MatchFulltext, PlanNodeCategory::DataAccess),
    // Vector Search Nodes
    #[cfg(feature = "qdrant")]
    (VectorSearch, PlanNodeCategory::DataAccess),
    #[cfg(feature = "qdrant")]
    (VectorLookup, PlanNodeCategory::DataAccess),
    #[cfg(feature = "qdrant")]
    (VectorMatch, PlanNodeCategory::DataAccess),
}

// Use macros to generate the describe method.
crate::define_enum_describe! {
    PlanNodeEnum,
    // Access node
    (Start, "Start"),
    (GetVertices, "GetVertices"),
    (GetEdges, "GetEdges"),
    (GetNeighbors, "GetNeighbors"),
    (ScanVertices, "ScanVertices"),
    (ScanEdges, "ScanEdges"),
    (IndexScan, "IndexScan"),
    // Operation node
    (Project, "Project"),
    (Filter, "Filter"),
    (Sort, "Sort"),
    (Limit, "Limit"),
    (TopN, "TopN"),
    (Sample, "Sample"),
    (Dedup, "Dedup"),
    (Aggregate, "Aggregate"),
    (Window, "Window function"),
    // Connecting nodes
    (InnerJoin, "InnerJoin"),
    (LeftJoin, "LeftJoin"),
    (RightJoin, "RightJoin"),
    (CrossJoin, "CrossJoin"),
    (FullOuterJoin, "FullOuterJoin"),
    (SemiJoin, "SemiJoin"),
    // Traverse the nodes
    (Expand, "Expand"),
    (ExpandAll, "ExpandAll"),
    (Traverse, "Traverse"),
    (AppendVertices, "AppendVertices"),
    (BiExpand, "BiExpand"),
    (BiTraverse, "BiTraverse"),
    // Control flow nodes
    (Argument, "Argument"),
    (Loop, "Loop"),
    (PassThrough, "PassThrough"),
    (Select, "Select"),
    // Transaction control nodes
    (BeginTransaction, "BeginTransaction"),
    (Commit, "Commit"),
    (Rollback, "Rollback"),
    (Savepoint, "Savepoint"),
    (ReleaseSavepoint, "ReleaseSavepoint"),
    // Data processing node
    (DataCollect, "DataCollect"),
    (Remove, "Remove"),
    (PatternApply, "PatternApply"),
    (RollUpApply, "RollUpApply"),
    (CorrelatedApply, "CorrelatedApply"),
    (Union, "Union"),
    (Minus, "Minus"),
    (Intersect, "Intersect"),
    (Unwind, "Unwind"),
    (Materialize, "Materialize"),
    (Assign, "Assign"),
    (Apply, "Apply"),
    // Algorithm node
    (MultiShortestPath, "MultiShortestPath"),
    (BFSShortest, "BFSShortest"),
    (AllPaths, "AllPaths"),
    (ShortestPath, "ShortestPath"),
    // Management Node (parameterized)
    (SpaceManage, "SpaceManage"),
    (TagManage, "TagManage"),
    (EdgeManage, "EdgeManage"),
    (IndexManage, "IndexManage"),
    (UserManage, "UserManage"),
    (FulltextManage, "FulltextManage"),
    (VectorManage, "VectorManage"),
    // Management Node – Data
    (InsertVertices, "InsertVertices"),
    (InsertEdges, "InsertEdges"),
    (DeleteVertices, "DeleteVertices"),
    (DeleteEdges, "DeleteEdges"),
    (DeleteTags, "DeleteTags"),
    (DeleteIndex, "DeleteIndex"),
    (PipeDeleteVertices, "PipeDeleteVertices"),
    (PipeDeleteEdges, "PipeDeleteEdges"),
    (Update, "Update"),
    (UpdateVertices, "UpdateVertices"),
    (UpdateEdges, "UpdateEdges"),
    // Statistical nodes
    (ShowStats, "ShowStats"),
    (ShowConfigs, "ShowConfigs"),
    (ShowQueries, "ShowQueries"),
    (ShowSessions, "ShowSessions"),
    // Full-text Search Nodes
    (FulltextSearch, "FulltextSearch"),
    (FulltextLookup, "FulltextLookup"),
    (MatchFulltext, "MatchFulltext"),
    // Vector Search Nodes
    #[cfg(feature = "qdrant")]
    (VectorSearch, "VectorSearch"),
    #[cfg(feature = "qdrant")]
    (VectorLookup, "VectorLookup"),
    #[cfg(feature = "qdrant")]
    (VectorMatch, "VectorMatch"),
}

impl PlanNodeEnum {
    /// Check if this node is any management node (parameterized or data modification)
    pub fn is_management(&self) -> bool {
        matches!(
            self,
            PlanNodeEnum::SpaceManage(_)
                | PlanNodeEnum::TagManage(_)
                | PlanNodeEnum::EdgeManage(_)
                | PlanNodeEnum::IndexManage(_)
                | PlanNodeEnum::UserManage(_)
                | PlanNodeEnum::FulltextManage(_)
                | PlanNodeEnum::VectorManage(_)
                | PlanNodeEnum::InsertVertices(_)
                | PlanNodeEnum::InsertEdges(_)
                | PlanNodeEnum::DeleteVertices(_)
                | PlanNodeEnum::DeleteEdges(_)
                | PlanNodeEnum::DeleteTags(_)
                | PlanNodeEnum::DeleteIndex(_)
                | PlanNodeEnum::PipeDeleteVertices(_)
                | PlanNodeEnum::PipeDeleteEdges(_)
                | PlanNodeEnum::Update(_)
                | PlanNodeEnum::UpdateVertices(_)
                | PlanNodeEnum::UpdateEdges(_)
                | PlanNodeEnum::ShowStats(_)
                | PlanNodeEnum::ShowConfigs(_)
                | PlanNodeEnum::ShowQueries(_)
                | PlanNodeEnum::ShowSessions(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Documented node-name list, mirrored from
    /// `__analysis__/plan_node_category_analysis.md`. The doc itself cannot be
    /// compile-checked, so this mirror is the only hand-maintained part; the
    /// registry side comes from the macro-generated `ALL_VARIANT_NAMES` above.
    ///
    /// When adding/removing a plan node you MUST update:
    /// 1. the enum variant + the six macro tables in this file (compiler-enforced),
    /// 2. this mirror list (test-enforced against the macro table),
    /// 3. the two docs in `__analysis__/`.
    #[cfg(not(feature = "qdrant"))]
    const DOCUMENTED_NAMES: &[&str] = &[
        // Access (7)
        "Start", "GetVertices", "GetEdges", "GetNeighbors", "ScanVertices", "ScanEdges",
        "IndexScan",
        // Operation (9)
        "Project", "Filter", "Sort", "Limit", "TopN", "Sample", "Dedup", "Aggregate", "Window",
        // Join (6)
        "InnerJoin", "LeftJoin", "RightJoin", "CrossJoin", "FullOuterJoin", "SemiJoin",
        // Traversal (6)
        "Expand", "ExpandAll", "Traverse", "AppendVertices", "BiExpand", "BiTraverse",
        // ControlFlow (9)
        "Argument", "Loop", "PassThrough", "Select", "BeginTransaction", "Commit", "Rollback",
        "Savepoint", "ReleaseSavepoint",
        // DataProcessing (12)
        "DataCollect", "Remove", "PatternApply", "RollUpApply", "CorrelatedApply", "Union",
        "Minus", "Intersect", "Unwind", "Materialize", "Assign", "Apply",
        // Algorithm (4)
        "MultiShortestPath", "BFSShortest", "AllPaths", "ShortestPath",
        // Management/DDL (22)
        "SpaceManage", "TagManage", "EdgeManage", "IndexManage", "UserManage", "FulltextManage",
        "VectorManage", "InsertVertices", "InsertEdges", "DeleteVertices", "DeleteEdges",
        "DeleteTags", "DeleteIndex", "PipeDeleteVertices", "PipeDeleteEdges", "Update",
        "UpdateVertices", "UpdateEdges", "ShowStats", "ShowConfigs", "ShowQueries", "ShowSessions",
        // DataAccess (3)
        "FulltextSearch", "FulltextLookup", "MatchFulltext",
    ];

    /// Same as above plus the three qdrant-gated vector nodes.
    #[cfg(feature = "qdrant")]
    const DOCUMENTED_NAMES: &[&str] = &[
        "Start", "GetVertices", "GetEdges", "GetNeighbors", "ScanVertices", "ScanEdges",
        "IndexScan", "Project", "Filter", "Sort", "Limit", "TopN", "Sample", "Dedup", "Aggregate",
        "Window", "InnerJoin", "LeftJoin", "RightJoin", "CrossJoin", "FullOuterJoin", "SemiJoin",
        "Expand", "ExpandAll", "Traverse", "AppendVertices", "BiExpand", "BiTraverse", "Argument",
        "Loop", "PassThrough", "Select", "BeginTransaction", "Commit", "Rollback", "Savepoint",
        "ReleaseSavepoint", "DataCollect", "Remove", "PatternApply", "RollUpApply",
        "CorrelatedApply", "Union", "Minus", "Intersect", "Unwind", "Materialize", "Assign",
        "Apply", "MultiShortestPath", "BFSShortest", "AllPaths", "ShortestPath", "SpaceManage",
        "TagManage", "EdgeManage", "IndexManage", "UserManage", "FulltextManage", "VectorManage",
        "InsertVertices", "InsertEdges", "DeleteVertices", "DeleteEdges", "DeleteTags",
        "DeleteIndex", "PipeDeleteVertices", "PipeDeleteEdges", "Update", "UpdateVertices",
        "UpdateEdges", "ShowStats", "ShowConfigs", "ShowQueries", "ShowSessions", "FulltextSearch",
        "FulltextLookup", "MatchFulltext", "VectorSearch", "VectorLookup", "VectorMatch",
    ];

    /// Default build: 78 variants. With `qdrant`: 81 variants.
    #[cfg(not(feature = "qdrant"))]
    const EXPECTED_VARIANT_COUNT: usize = 78;
    #[cfg(feature = "qdrant")]
    const EXPECTED_VARIANT_COUNT: usize = 81;

    #[test]
    fn variant_count_matches_documented_number() {
        assert_eq!(
            PlanNodeEnum::ALL_VARIANT_NAMES.len(),
            EXPECTED_VARIANT_COUNT,
            "PlanNodeEnum variant count drifted from the documented {EXPECTED_VARIANT_COUNT}. \
             Update the macro tables / enum in this file and the __analysis__ docs."
        );
    }

    #[test]
    fn variant_names_are_unique() {
        let mut seen = HashSet::new();
        for name in PlanNodeEnum::ALL_VARIANT_NAMES {
            assert!(seen.insert(*name), "duplicate variant name: {name}");
        }
    }

    #[test]
    fn macro_registry_matches_documented_names() {
        let from_macro: HashSet<&str> = PlanNodeEnum::ALL_VARIANT_NAMES.iter().copied().collect();
        let documented: HashSet<&str> = DOCUMENTED_NAMES.iter().copied().collect();
        assert_eq!(
            from_macro, documented,
            "macro registry and __analysis__ documented list drifted. \
             Missing in macro table: {:?}. Extraneous in macro table: {:?}.",
            documented.difference(&from_macro).collect::<Vec<_>>(),
            from_macro.difference(&documented).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn removed_legacy_variants_are_absent() {
        for legacy in ["HashInnerJoin", "HashLeftJoin", "FulltextIndexScan"] {
            assert!(
                !PlanNodeEnum::ALL_VARIANT_NAMES.contains(&legacy),
                "removed legacy variant {legacy} is still registered"
            );
        }
    }

    #[test]
    fn vector_search_nodes_are_feature_gated() {
        let names = PlanNodeEnum::ALL_VARIANT_NAMES;
        // VectorManage is an always-present management node; the three vector
        // SEARCH nodes are the qdrant-gated ones.
        let vector_search: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| matches!(*n, "VectorSearch" | "VectorLookup" | "VectorMatch"))
            .collect();
        #[cfg(feature = "qdrant")]
        {
            assert_eq!(vector_search.len(), 3, "expected 3 vector search nodes under qdrant");
            for n in ["VectorSearch", "VectorLookup", "VectorMatch"] {
                assert!(vector_search.contains(&n), "missing {n} under qdrant");
            }
        }
        #[cfg(not(feature = "qdrant"))]
        {
            assert!(
                vector_search.is_empty(),
                "vector search nodes leaked into the default build: {vector_search:?}"
            );
        }
    }

    #[test]
    fn type_name_consistent_with_registry() {
        // `type_name()` is generated from the same macro table as
        // `ALL_VARIANT_NAMES`; spot-check the default instance to keep the
        // wiring honest.
        let node = PlanNodeEnum::default();
        assert!(PlanNodeEnum::ALL_VARIANT_NAMES.contains(&node.type_name()));
    }
}
