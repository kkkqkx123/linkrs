use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::QueryContext;

mod access;
mod algorithm;
mod control_flow;
mod data_processing;
mod dml;
mod join;
mod operation;
mod search;
mod traversal;

use access::*;
use algorithm::*;
use control_flow::*;
use data_processing::*;
use dml::*;
use join::*;
use operation::*;
use search::*;
use traversal::*;

pub trait PhysicalPlanner: Send + Sync + std::fmt::Debug {
    fn plan(&self, logical: LogicalNodeEnum, qctx: &QueryContext) -> PlanNodeEnum;
}

#[derive(Debug)]
pub struct DefaultPhysicalPlanner;

impl Default for DefaultPhysicalPlanner {
    fn default() -> Self {
        Self
    }
}

impl DefaultPhysicalPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl PhysicalPlanner for DefaultPhysicalPlanner {
    fn plan(&self, logical: LogicalNodeEnum, _qctx: &QueryContext) -> PlanNodeEnum {
        convert_logical_to_physical(logical)
    }
}

pub(crate) fn convert_logical_to_physical(logical: LogicalNodeEnum) -> PlanNodeEnum {
    match logical {
        // ==================== Access Nodes ====================
        LogicalNodeEnum::Start(n) => convert_start(n),

        LogicalNodeEnum::GetVertices(n) => convert_get_vertices(n),

        LogicalNodeEnum::GetEdges(n) => convert_get_edges(n),

        LogicalNodeEnum::GetNeighbors(n) => convert_get_neighbors(n),

        LogicalNodeEnum::ScanVertices(n) => convert_scan_vertices(n),

        LogicalNodeEnum::ScanEdges(n) => convert_scan_edges(n),

        // ==================== Operation Nodes ====================
        LogicalNodeEnum::Project(n) => convert_project(n),

        LogicalNodeEnum::Filter(n) => convert_filter(n),

        LogicalNodeEnum::Sort(n) => convert_sort(n),

        LogicalNodeEnum::Limit(n) => convert_limit(n),

        LogicalNodeEnum::Skip(n) => convert_skip(n),

        LogicalNodeEnum::TopN(n) => convert_top_n(n),

        LogicalNodeEnum::Sample(n) => convert_sample(n),

        LogicalNodeEnum::Dedup(n) => convert_dedup(n),

        LogicalNodeEnum::Aggregate(n) => convert_aggregate(n),

        LogicalNodeEnum::Window(n) => convert_window(n),

        // ==================== Join Nodes ====================
        //
        // The converter produces only the logical join variants
        // (InnerJoin/LeftJoin); hash keys remain attached to them and the
        // arena builder decides the physical hash vs nested-loop algorithm.
        LogicalNodeEnum::InnerJoin(n) => convert_inner_join(n),

        LogicalNodeEnum::LeftJoin(n) => convert_left_join(n),

        LogicalNodeEnum::RightJoin(n) => convert_right_join(n),

        LogicalNodeEnum::CrossJoin(n) => convert_cross_join(n),

        LogicalNodeEnum::FullOuterJoin(n) => convert_full_outer_join(n),

        LogicalNodeEnum::SemiJoin(n) => convert_semi_join(n),

        // ==================== Traversal Nodes ====================
        LogicalNodeEnum::Expand(n) => convert_expand(n),

        LogicalNodeEnum::ExpandAll(n) => convert_expand_all(n),

        LogicalNodeEnum::Traverse(n) => convert_traverse(n),

        LogicalNodeEnum::AppendVertices(n) => convert_append_vertices(n),

        LogicalNodeEnum::BiExpand(n) => convert_bi_expand(n),

        LogicalNodeEnum::BiTraverse(n) => convert_bi_traverse(n),

        // ==================== Control Flow Nodes ====================
        LogicalNodeEnum::Argument(n) => convert_argument(n),

        LogicalNodeEnum::Loop(n) => convert_loop(n),

        LogicalNodeEnum::PassThrough(n) => convert_pass_through(n),

        LogicalNodeEnum::Select(n) => convert_select(n),

        LogicalNodeEnum::BeginTransaction(n) => convert_begin_transaction(n),

        LogicalNodeEnum::Commit(n) => convert_commit(n),

        LogicalNodeEnum::Rollback(n) => convert_rollback(n),

        // ==================== Data Processing Nodes ====================
        LogicalNodeEnum::DataCollect(n) => convert_data_collect(n),

        LogicalNodeEnum::Remove(n) => convert_remove(n),

        LogicalNodeEnum::PatternApply(n) => convert_pattern_apply(n),

        LogicalNodeEnum::CorrelatedApply(n) => convert_correlated_apply(n),

        LogicalNodeEnum::RollUpApply(n) => convert_roll_up_apply(n),

        LogicalNodeEnum::Union(n) => convert_union(n),

        LogicalNodeEnum::Minus(n) => convert_minus(n),

        LogicalNodeEnum::Intersect(n) => convert_intersect(n),

        LogicalNodeEnum::Unwind(n) => convert_unwind(n),

        LogicalNodeEnum::Materialize(n) => convert_materialize(n),

        LogicalNodeEnum::Assign(n) => convert_assign(n),

        LogicalNodeEnum::Apply(n) => convert_apply(n),

        // ==================== Algorithm Nodes ====================
        LogicalNodeEnum::MultiShortestPath(n) => convert_multi_shortest_path(n),

        LogicalNodeEnum::BFSShortest(n) => convert_b_f_s_shortest(n),

        LogicalNodeEnum::AllPaths(n) => convert_all_paths(n),

        LogicalNodeEnum::ShortestPath(n) => convert_shortest_path(n),

        // ==================== Search Nodes ====================
        LogicalNodeEnum::FulltextSearch(n) => convert_fulltext_search(n),

        LogicalNodeEnum::FulltextLookup(n) => convert_fulltext_lookup(n),

        LogicalNodeEnum::MatchFulltext(n) => convert_match_fulltext(n),

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorSearch(n) => convert_simvec(n),

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorLookup(n) => convert_vector_lookup(n),

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorMatch(n) => convert_vector_match(n),

        LogicalNodeEnum::Flatten(n) => convert_flatten(n),

        LogicalNodeEnum::WcoIntersect(n) => lower_wco_intersect(n),

        LogicalNodeEnum::InsertVertices(n) => convert_insert_vertices(n),

        LogicalNodeEnum::InsertEdges(n) => convert_insert_edges(n),

        LogicalNodeEnum::Update(n) => convert_update(n),

        LogicalNodeEnum::DeleteVertices(n) => convert_delete_vertices(n),

        LogicalNodeEnum::DeleteEdges(n) => convert_delete_edges(n),

        LogicalNodeEnum::DeleteIndex(n) => convert_delete_index(n),

        LogicalNodeEnum::PipeDeleteVertices(n) => convert_pipe_delete_vertices(n),

        LogicalNodeEnum::PipeDeleteEdges(n) => convert_pipe_delete_edges(n),

        LogicalNodeEnum::CopyFrom(n) => convert_copy_from(n),

        LogicalNodeEnum::CopyTo(n) => convert_copy_to(n),
    }
}
