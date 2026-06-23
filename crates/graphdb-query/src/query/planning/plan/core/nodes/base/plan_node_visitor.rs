//! Implementation of the PlanNode visitor pattern

use super::plan_node_enum::PlanNodeEnum;
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
use crate::query::planning::plan::core::nodes::search::fulltext::data_access::{
    FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
};
#[cfg(feature = "qdrant")]
use crate::query::planning::plan::core::nodes::search::vector::data_access::{
    VectorLookupNode, VectorMatchNode, VectorSearchNode,
};

pub use crate::query::planning::plan::core::nodes::access::graph_scan_node::{
    EdgeIndexScanNode, GetEdgesNode, GetNeighborsNode, GetVerticesNode, ScanEdgesNode,
    ScanVerticesNode,
};
pub use crate::query::planning::plan::core::nodes::access::index_scan::IndexScanNode;
pub use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::{
    ArgumentNode, BeginTransactionNode, CommitNode, LoopNode, PassThroughNode, RollbackNode,
    SelectNode,
};
pub use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
pub use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
pub use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    ApplyNode, AssignNode, DataCollectNode, DedupNode, MaterializeNode, PatternApplyNode,
    RemoveNode, RollUpApplyNode, UnionNode, UnwindNode,
};
pub use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::{
    IntersectNode, MinusNode,
};
pub use crate::query::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, FullOuterJoinNode, HashInnerJoinNode, HashLeftJoinNode, InnerJoinNode,
    LeftJoinNode, RightJoinNode, SemiJoinNode,
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

macro_rules! impl_visitor_methods {
    ($($name:ident, $node_type:ty, $visit_method:ident);* $(;)?) => {
        $(
            fn $visit_method(&mut self, node: &$node_type) -> Self::Result {
                let _ = node;
                self.visit_default()
            }
        )*
    };
}

pub trait PlanNodeVisitor {
    type Result;

    fn visit_default(&mut self) -> Self::Result;

    impl_visitor_methods!(
        Start, StartNode, visit_start;
        Project, ProjectNode, visit_project;
        Sort, SortNode, visit_sort;
        Limit, LimitNode, visit_limit;
        TopN, TopNNode, visit_topn;
        Sample, SampleNode, visit_sample;
    );

    impl_visitor_methods!(
        InnerJoin, InnerJoinNode, visit_inner_join;
        LeftJoin, LeftJoinNode, visit_left_join;
        RightJoin, RightJoinNode, visit_right_join;
        CrossJoin, CrossJoinNode, visit_cross_join;
        HashInnerJoin, HashInnerJoinNode, visit_hash_inner_join;
        HashLeftJoin, HashLeftJoinNode, visit_hash_left_join;
        FullOuterJoin, FullOuterJoinNode, visit_full_outer_join;
        SemiJoin, SemiJoinNode, visit_semi_join;
    );

    impl_visitor_methods!(
        GetVertices, GetVerticesNode, visit_get_vertices;
        GetEdges, GetEdgesNode, visit_get_edges;
        GetNeighbors, GetNeighborsNode, visit_get_neighbors;
        ScanVertices, ScanVerticesNode, visit_scan_vertices;
        ScanEdges, ScanEdgesNode, visit_scan_edges;
        EdgeIndexScan, EdgeIndexScanNode, visit_edge_index_scan;
    );

    impl_visitor_methods!(
        Expand, ExpandNode, visit_expand;
        ExpandAll, ExpandAllNode, visit_expand_all;
        Traverse, TraverseNode, visit_traverse;
        AppendVertices, AppendVerticesNode, visit_append_vertices;
        BiExpand, BiExpandNode, visit_bi_expand;
        BiTraverse, BiTraverseNode, visit_bi_traverse;
    );

    impl_visitor_methods!(
        Filter, FilterNode, visit_filter;
        Aggregate, AggregateNode, visit_aggregate;
        Dedup, DedupNode, visit_dedup;
    );

    impl_visitor_methods!(
        Argument, ArgumentNode, visit_argument;
        Loop, LoopNode, visit_loop;
        PassThrough, PassThroughNode, visit_pass_through;
        Select, SelectNode, visit_select;
        BeginTransaction, BeginTransactionNode, visit_begin_transaction;
        Commit, CommitNode, visit_commit;
        Rollback, RollbackNode, visit_rollback;
        DataCollect, DataCollectNode, visit_data_collect;
    );

    impl_visitor_methods!(
        PatternApply, PatternApplyNode, visit_pattern_apply;
        RollUpApply, RollUpApplyNode, visit_roll_up_apply;
        Remove, RemoveNode, visit_remove;
    );

    impl_visitor_methods!(
        Union, UnionNode, visit_union;
        Minus, MinusNode, visit_minus;
        Intersect, IntersectNode, visit_intersect;
        Unwind, UnwindNode, visit_unwind;
        Materialize, MaterializeNode, visit_materialize;
        Assign, AssignNode, visit_assign;
        Apply, ApplyNode, visit_apply;
    );

    impl_visitor_methods!(
        IndexScan, IndexScanNode, visit_index_scan;
        MultiShortestPath, MultiShortestPathNode, visit_multi_shortest_path;
        BFSShortest, BFSShortestNode, visit_bfs_shortest;
        AllPaths, AllPathsNode, visit_all_paths;
        ShortestPath, ShortestPathNode, visit_shortest_path;
    );

    impl_visitor_methods!(
        SpaceManage, SpaceManageNode, visit_space_manage;
        TagManage, TagManageNode, visit_tag_manage;
        EdgeManage, EdgeManageNode, visit_edge_manage;
        IndexManage, IndexManageNode, visit_index_manage;
        UserManage, UserManageNode, visit_user_manage;
        FulltextManage, FulltextManageNode, visit_fulltext_manage;
        VectorManage, VectorManageNode, visit_vector_manage;
    );

    impl_visitor_methods!(
        ShowStats, ShowStatsNode, visit_show_stats;
    );

    impl_visitor_methods!(
        InsertVertices, InsertVerticesNode, visit_insert_vertices;
        InsertEdges, InsertEdgesNode, visit_insert_edges;
    );

    impl_visitor_methods!(
        DeleteVertices, DeleteVerticesNode, visit_delete_vertices;
        DeleteEdges, DeleteEdgesNode, visit_delete_edges;
        DeleteTags, DeleteTagsNode, visit_delete_tags;
        DeleteIndex, DeleteIndexNode, visit_delete_index;
    );

    impl_visitor_methods!(
        PipeDeleteVertices, PipeDeleteVerticesNode, visit_pipe_delete_vertices;
        PipeDeleteEdges, PipeDeleteEdgesNode, visit_pipe_delete_edges;
    );

    impl_visitor_methods!(
        Update, UpdateNode, visit_update;
        UpdateVertices, UpdateVerticesNode, visit_update_vertices;
        UpdateEdges, UpdateEdgesNode, visit_update_edges;
    );

    impl_visitor_methods!(
        FulltextSearch, FulltextSearchNode, visit_fulltext_search;
        FulltextLookup, FulltextLookupNode, visit_fulltext_lookup;
        MatchFulltext, MatchFulltextNode, visit_match_fulltext;
    );

    #[cfg(feature = "qdrant")]
    impl_visitor_methods!(
        VectorSearch, VectorSearchNode, visit_vector_search;
        VectorLookup, VectorLookupNode, visit_vector_lookup;
        VectorMatch, VectorMatchNode, visit_vector_match;
    );
}

impl PlanNodeEnum {
    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: PlanNodeVisitor,
    {
        match self {
            PlanNodeEnum::Start(node) => visitor.visit_start(node),
            PlanNodeEnum::Project(node) => visitor.visit_project(node),
            PlanNodeEnum::Sort(node) => visitor.visit_sort(node),
            PlanNodeEnum::Limit(node) => visitor.visit_limit(node),
            PlanNodeEnum::TopN(node) => visitor.visit_topn(node),
            PlanNodeEnum::Sample(node) => visitor.visit_sample(node),
            PlanNodeEnum::InnerJoin(node) => visitor.visit_inner_join(node),
            PlanNodeEnum::LeftJoin(node) => visitor.visit_left_join(node),
            PlanNodeEnum::RightJoin(node) => visitor.visit_right_join(node),
            PlanNodeEnum::CrossJoin(node) => visitor.visit_cross_join(node),
            PlanNodeEnum::SemiJoin(node) => visitor.visit_semi_join(node),
            PlanNodeEnum::GetVertices(node) => visitor.visit_get_vertices(node),
            PlanNodeEnum::GetEdges(node) => visitor.visit_get_edges(node),
            PlanNodeEnum::GetNeighbors(node) => visitor.visit_get_neighbors(node),
            PlanNodeEnum::ScanVertices(node) => visitor.visit_scan_vertices(node),
            PlanNodeEnum::ScanEdges(node) => visitor.visit_scan_edges(node),
            PlanNodeEnum::EdgeIndexScan(node) => visitor.visit_edge_index_scan(node),
            PlanNodeEnum::HashInnerJoin(node) => visitor.visit_hash_inner_join(node),
            PlanNodeEnum::HashLeftJoin(node) => visitor.visit_hash_left_join(node),
            PlanNodeEnum::FullOuterJoin(node) => visitor.visit_full_outer_join(node),
            PlanNodeEnum::Expand(node) => visitor.visit_expand(node),
            PlanNodeEnum::ExpandAll(node) => visitor.visit_expand_all(node),
            PlanNodeEnum::Traverse(node) => visitor.visit_traverse(node),
            PlanNodeEnum::AppendVertices(node) => visitor.visit_append_vertices(node),
            PlanNodeEnum::BiExpand(node) => visitor.visit_bi_expand(node),
            PlanNodeEnum::BiTraverse(node) => visitor.visit_bi_traverse(node),
            PlanNodeEnum::Filter(node) => visitor.visit_filter(node),
            PlanNodeEnum::Aggregate(node) => visitor.visit_aggregate(node),
            PlanNodeEnum::Argument(node) => visitor.visit_argument(node),
            PlanNodeEnum::Loop(node) => visitor.visit_loop(node),
            PlanNodeEnum::PassThrough(node) => visitor.visit_pass_through(node),
            PlanNodeEnum::Select(node) => visitor.visit_select(node),
            PlanNodeEnum::BeginTransaction(node) => visitor.visit_begin_transaction(node),
            PlanNodeEnum::Commit(node) => visitor.visit_commit(node),
            PlanNodeEnum::Rollback(node) => visitor.visit_rollback(node),
            PlanNodeEnum::DataCollect(node) => visitor.visit_data_collect(node),
            PlanNodeEnum::Dedup(node) => visitor.visit_dedup(node),
            PlanNodeEnum::PatternApply(node) => visitor.visit_pattern_apply(node),
            PlanNodeEnum::RollUpApply(node) => visitor.visit_roll_up_apply(node),
            PlanNodeEnum::Remove(node) => visitor.visit_remove(node),
            PlanNodeEnum::Union(node) => visitor.visit_union(node),
            PlanNodeEnum::Minus(node) => visitor.visit_minus(node),
            PlanNodeEnum::Intersect(node) => visitor.visit_intersect(node),
            PlanNodeEnum::Unwind(node) => visitor.visit_unwind(node),
            PlanNodeEnum::Materialize(node) => visitor.visit_materialize(node),
            PlanNodeEnum::Assign(node) => visitor.visit_assign(node),
            PlanNodeEnum::Apply(node) => visitor.visit_apply(node),
            PlanNodeEnum::IndexScan(node) => visitor.visit_index_scan(node),
            PlanNodeEnum::MultiShortestPath(node) => visitor.visit_multi_shortest_path(node),
            PlanNodeEnum::BFSShortest(node) => visitor.visit_bfs_shortest(node),
            PlanNodeEnum::AllPaths(node) => visitor.visit_all_paths(node),
            PlanNodeEnum::ShortestPath(node) => visitor.visit_shortest_path(node),

            PlanNodeEnum::SpaceManage(node) => visitor.visit_space_manage(node),
            PlanNodeEnum::TagManage(node) => visitor.visit_tag_manage(node),
            PlanNodeEnum::EdgeManage(node) => visitor.visit_edge_manage(node),
            PlanNodeEnum::IndexManage(node) => visitor.visit_index_manage(node),
            PlanNodeEnum::UserManage(node) => visitor.visit_user_manage(node),
            PlanNodeEnum::FulltextManage(node) => visitor.visit_fulltext_manage(node),
            PlanNodeEnum::VectorManage(node) => visitor.visit_vector_manage(node),

            PlanNodeEnum::ShowStats(node) => visitor.visit_show_stats(node),
            PlanNodeEnum::InsertVertices(node) => visitor.visit_insert_vertices(node),
            PlanNodeEnum::InsertEdges(node) => visitor.visit_insert_edges(node),
            PlanNodeEnum::DeleteVertices(node) => visitor.visit_delete_vertices(node),
            PlanNodeEnum::DeleteEdges(node) => visitor.visit_delete_edges(node),
            PlanNodeEnum::DeleteTags(node) => visitor.visit_delete_tags(node),
            PlanNodeEnum::DeleteIndex(node) => visitor.visit_delete_index(node),
            PlanNodeEnum::PipeDeleteVertices(node) => visitor.visit_pipe_delete_vertices(node),
            PlanNodeEnum::PipeDeleteEdges(node) => visitor.visit_pipe_delete_edges(node),
            PlanNodeEnum::Update(node) => visitor.visit_update(node),
            PlanNodeEnum::UpdateVertices(node) => visitor.visit_update_vertices(node),
            PlanNodeEnum::UpdateEdges(node) => visitor.visit_update_edges(node),

            PlanNodeEnum::FulltextSearch(node) => visitor.visit_fulltext_search(node),
            PlanNodeEnum::FulltextLookup(node) => visitor.visit_fulltext_lookup(node),
            PlanNodeEnum::MatchFulltext(node) => visitor.visit_match_fulltext(node),
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(node) => visitor.visit_vector_search(node),
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorLookup(node) => visitor.visit_vector_lookup(node),
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorMatch(node) => visitor.visit_vector_match(node),
        }
    }
}
