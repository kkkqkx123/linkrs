//! Executor enumeration definition
//!
//! Use static distribution instead of dynamic distribution; all types of executors are included in this enumeration.
//! By implementing the Executor trait for enumerations, it is possible to handle all types of executors in a unified manner.

use std::fmt;
use std::fmt::{Debug, Formatter};

use crate::storage::StorageClient;

use crate::query::executor::admin::AnalyzeExecutor;
#[cfg(feature = "fulltext-search")]
use crate::query::executor::base::FulltextManageExecutor;
#[cfg(feature = "qdrant")]
use crate::query::executor::base::VectorManageExecutor;
use crate::query::executor::base::{
    BaseExecutor, DBResult, EdgeManageExecutor, ExecutionResult, Executor, ExecutorStats,
    IndexManageExecutor, InputExecutor, SpaceManageExecutor, StartExecutor, TagManageExecutor,
    UserManageExecutor,
};
use crate::query::executor::control_flow::{
    ForLoopExecutor, LoopExecutor, SelectExecutor, WhileLoopExecutor,
};
#[cfg(feature = "fulltext-search")]
use crate::query::executor::data_access::{
    FulltextScanExecutor, FulltextSearchExecutor, MatchFulltextExecutor,
};
use crate::query::executor::data_access::{
    GetEdgesExecutor, GetNeighborsExecutor, GetPropExecutor, GetVerticesExecutor,
    IndexScanExecutor, ScanEdgesExecutor, ScanVerticesExecutor,
};
#[cfg(feature = "qdrant")]
use crate::query::executor::data_access::{
    VectorLookupExecutor, VectorMatchExecutor, VectorSearchExecutor,
};
use crate::query::executor::data_modification::{
    DeleteExecutor, InsertExecutor, PipeDeleteExecutor, RemoveExecutor, UpdateExecutor,
};
use crate::query::executor::graph_operations::graph_traversal::algorithms::BFSShortestExecutor;
use crate::query::executor::graph_operations::graph_traversal::{
    algorithms::MultiShortestPathExecutor, AllPathsExecutor, ExpandAllExecutor, ExpandExecutor,
    ShortestPathExecutor, TraverseExecutor,
};
use crate::query::executor::graph_operations::MaterializeExecutor;
use crate::query::executor::relational_algebra::join::{
    CrossJoinExecutor, FullOuterJoinExecutor, HashInnerJoinExecutor, HashLeftJoinExecutor,
    InnerJoinExecutor, LeftJoinExecutor,
};
use crate::query::executor::relational_algebra::set_operations::{
    IntersectExecutor, MinusExecutor, UnionAllExecutor, UnionExecutor,
};
use crate::query::executor::relational_algebra::{
    AggregateExecutor, FilterExecutor, GroupByExecutor, HavingExecutor, ProjectExecutor,
};
use crate::query::executor::result_processing::transformations::{
    AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor, RollUpApplyExecutor,
    UnwindExecutor,
};
use crate::query::executor::result_processing::{
    DedupExecutor, LimitExecutor, SampleExecutor, SortExecutor, TopNExecutor,
};
use crate::query::executor::utils::{ArgumentExecutor, DataCollectExecutor, PassThroughExecutor};

/// Executor enumeration
///
/// Include all possible types of executors, and implement polymorphism using static distribution.
pub enum ExecutorEnum<S: StorageClient + Send + 'static> {
    Start(StartExecutor<S>),
    Base(BaseExecutor<S>),
    GetVertices(GetVerticesExecutor<S>),
    GetEdges(GetEdgesExecutor<S>),
    GetNeighbors(GetNeighborsExecutor<S>),
    GetProp(GetPropExecutor<S>),
    AllPaths(AllPathsExecutor<S>),
    Expand(ExpandExecutor<S>),
    ExpandAll(ExpandAllExecutor<S>),
    Traverse(TraverseExecutor<S>),
    BiExpand(ExpandExecutor<S>),
    BiTraverse(ExpandExecutor<S>),
    ShortestPath(ShortestPathExecutor<S>),
    MultiShortestPath(MultiShortestPathExecutor<S>),
    InnerJoin(InnerJoinExecutor<S>),
    HashInnerJoin(HashInnerJoinExecutor<S>),
    LeftJoin(LeftJoinExecutor<S>),
    HashLeftJoin(HashLeftJoinExecutor<S>),
    FullOuterJoin(FullOuterJoinExecutor<S>),
    CrossJoin(CrossJoinExecutor<S>),
    Union(UnionExecutor<S>),
    UnionAll(UnionAllExecutor<S>),
    Minus(MinusExecutor<S>),
    Intersect(IntersectExecutor<S>),
    Filter(FilterExecutor<S>),
    Project(ProjectExecutor<S>),
    Limit(LimitExecutor<S>),
    Sort(SortExecutor<S>),
    TopN(TopNExecutor<S>),
    Sample(SampleExecutor<S>),
    Aggregate(AggregateExecutor<S>),
    GroupBy(GroupByExecutor<S>),
    Having(HavingExecutor<S>),
    Dedup(DedupExecutor<S>),
    Unwind(UnwindExecutor<S>),
    Assign(AssignExecutor<S>),
    Materialize(MaterializeExecutor<S>),
    AppendVertices(AppendVerticesExecutor<S>),
    RollUpApply(RollUpApplyExecutor<S>),
    PatternApply(PatternApplyExecutor<S>),
    Remove(RemoveExecutor<S>),
    Delete(DeleteExecutor<S>),
    PipeDelete(PipeDeleteExecutor<S>),
    Update(UpdateExecutor<S>),
    InsertVertices(InsertExecutor<S>),
    InsertEdges(InsertExecutor<S>),
    Loop(LoopExecutor<S>),
    ForLoop(ForLoopExecutor<S>),
    WhileLoop(WhileLoopExecutor<S>),
    Select(SelectExecutor<S>),
    ScanEdges(ScanEdgesExecutor<S>),
    ScanVertices(ScanVerticesExecutor<S>),
    IndexScan(IndexScanExecutor<S>),
    Argument(ArgumentExecutor<S>),
    PassThrough(PassThroughExecutor<S>),
    DataCollect(DataCollectExecutor<S>),
    BFSShortest(BFSShortestExecutor<S>),

    // ========== Management Executors (parameterized) ==========
    SpaceManage(SpaceManageExecutor<S>),
    TagManage(TagManageExecutor<S>),
    EdgeManage(EdgeManageExecutor<S>),
    IndexManage(IndexManageExecutor<S>),
    UserManage(UserManageExecutor<S>),
    #[cfg(feature = "fulltext-search")]
    FulltextManage(FulltextManageExecutor<S>),
    #[cfg(feature = "qdrant")]
    VectorManage(VectorManageExecutor<S>),

    // Statistics
    ShowStats(crate::query::executor::admin::query_management::show_stats::ShowStatsExecutor<S>),
    Analyze(AnalyzeExecutor<S>),

    // Full-text Search Executors (data access)
    #[cfg(feature = "fulltext-search")]
    FulltextSearch(FulltextSearchExecutor<S>),
    #[cfg(feature = "fulltext-search")]
    FulltextLookup(FulltextScanExecutor<S>),
    #[cfg(feature = "fulltext-search")]
    MatchFulltext(MatchFulltextExecutor<S>),

    // Vector Search Executors (data access)
    #[cfg(feature = "qdrant")]
    VectorSearch(VectorSearchExecutor<S>),
    #[cfg(feature = "qdrant")]
    VectorLookup(VectorLookupExecutor<S>),
    #[cfg(feature = "qdrant")]
    VectorMatch(VectorMatchExecutor<S>),
}

impl<S: StorageClient + Send + 'static> Debug for ExecutorEnum<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (variant_name, exec_name) = match self {
            ExecutorEnum::Start(exec) => ("Start", exec.name()),
            ExecutorEnum::Base(exec) => ("Base", exec.name()),
            ExecutorEnum::GetVertices(exec) => ("GetVertices", exec.name()),
            ExecutorEnum::GetEdges(exec) => ("GetEdges", exec.name()),
            ExecutorEnum::GetNeighbors(exec) => ("GetNeighbors", exec.name()),
            ExecutorEnum::GetProp(exec) => ("GetProp", exec.name()),
            ExecutorEnum::AllPaths(exec) => ("AllPaths", exec.name()),
            ExecutorEnum::Expand(exec) => ("Expand", exec.name()),
            ExecutorEnum::ExpandAll(exec) => ("ExpandAll", exec.name()),
            ExecutorEnum::Traverse(exec) => ("Traverse", exec.name()),
            ExecutorEnum::BiExpand(exec) => ("BiExpand", exec.name()),
            ExecutorEnum::BiTraverse(exec) => ("BiTraverse", exec.name()),
            ExecutorEnum::ShortestPath(exec) => ("ShortestPath", exec.name()),
            ExecutorEnum::MultiShortestPath(exec) => ("MultiShortestPath", exec.name()),
            ExecutorEnum::InnerJoin(exec) => ("InnerJoin", exec.name()),
            ExecutorEnum::HashInnerJoin(exec) => ("HashInnerJoin", exec.name()),
            ExecutorEnum::LeftJoin(exec) => ("LeftJoin", exec.name()),
            ExecutorEnum::HashLeftJoin(exec) => ("HashLeftJoin", exec.name()),
            ExecutorEnum::FullOuterJoin(exec) => ("FullOuterJoin", exec.name()),
            ExecutorEnum::CrossJoin(exec) => ("CrossJoin", exec.name()),
            ExecutorEnum::Union(exec) => ("Union", exec.name()),
            ExecutorEnum::UnionAll(exec) => ("UnionAll", exec.name()),
            ExecutorEnum::Minus(exec) => ("Minus", exec.name()),
            ExecutorEnum::Intersect(exec) => ("Intersect", exec.name()),
            ExecutorEnum::Filter(exec) => ("Filter", exec.name()),
            ExecutorEnum::Project(exec) => ("Project", exec.name()),
            ExecutorEnum::Limit(exec) => ("Limit", exec.name()),
            ExecutorEnum::Sort(exec) => ("Sort", exec.name()),
            ExecutorEnum::TopN(exec) => ("TopN", exec.name()),
            ExecutorEnum::Sample(exec) => ("Sample", exec.name()),
            ExecutorEnum::Aggregate(exec) => ("Aggregate", exec.name()),
            ExecutorEnum::GroupBy(exec) => ("GroupBy", exec.name()),
            ExecutorEnum::Having(exec) => ("Having", exec.name()),
            ExecutorEnum::Dedup(exec) => ("Dedup", exec.name()),
            ExecutorEnum::Unwind(exec) => ("Unwind", exec.name()),
            ExecutorEnum::Assign(exec) => ("Assign", exec.name()),
            ExecutorEnum::Materialize(exec) => ("Materialize", exec.name()),
            ExecutorEnum::AppendVertices(exec) => ("AppendVertices", exec.name()),
            ExecutorEnum::RollUpApply(exec) => ("RollUpApply", exec.name()),
            ExecutorEnum::PatternApply(exec) => ("PatternApply", exec.name()),
            ExecutorEnum::Remove(exec) => ("Remove", exec.name()),
            ExecutorEnum::Delete(exec) => ("Delete", exec.name()),
            ExecutorEnum::PipeDelete(exec) => ("PipeDelete", exec.name()),
            ExecutorEnum::Update(exec) => ("Update", exec.name()),
            ExecutorEnum::InsertVertices(exec) => ("InsertVertices", exec.name()),
            ExecutorEnum::InsertEdges(exec) => ("InsertEdges", exec.name()),
            ExecutorEnum::Loop(exec) => ("Loop", exec.name()),
            ExecutorEnum::ForLoop(exec) => ("ForLoop", exec.name()),
            ExecutorEnum::WhileLoop(exec) => ("WhileLoop", exec.name()),
            ExecutorEnum::Select(exec) => ("Select", exec.name()),
            ExecutorEnum::ScanEdges(exec) => ("ScanEdges", exec.name()),
            ExecutorEnum::ScanVertices(exec) => ("ScanVertices", exec.name()),
            ExecutorEnum::IndexScan(exec) => ("IndexScan", exec.name()),
            ExecutorEnum::Argument(exec) => ("Argument", exec.name()),
            ExecutorEnum::PassThrough(exec) => ("PassThrough", exec.name()),
            ExecutorEnum::DataCollect(exec) => ("DataCollect", exec.name()),
            ExecutorEnum::BFSShortest(exec) => ("BFSShortest", exec.name()),
            // Management Executors (parameterized)
            ExecutorEnum::SpaceManage(exec) => ("SpaceManage", exec.name()),
            ExecutorEnum::TagManage(exec) => ("TagManage", exec.name()),
            ExecutorEnum::EdgeManage(exec) => ("EdgeManage", exec.name()),
            ExecutorEnum::IndexManage(exec) => ("IndexManage", exec.name()),
            ExecutorEnum::UserManage(exec) => ("UserManage", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(exec) => ("FulltextManage", exec.name()),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(exec) => ("VectorManage", exec.name()),
            // Statistics
            ExecutorEnum::ShowStats(exec) => ("ShowStats", exec.name()),
            ExecutorEnum::Analyze(exec) => ("Analyze", exec.name()),
            // Full-text Search Executors (data access)
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(exec) => ("FulltextSearch", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(exec) => ("FulltextLookup", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(exec) => ("MatchFulltext", exec.name()),
            // Vector Search Executors (data access)
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorSearch(exec) => ("VectorSearch", exec.name()),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorLookup(exec) => ("VectorLookup", exec.name()),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorMatch(exec) => ("VectorMatch", exec.name()),
        };
        f.write_str(&format!("ExecutorEnum::{}({})", variant_name, exec_name))
    }
}

impl<S: StorageClient + Send + 'static> ExecutorEnum<S> {
    pub fn id(&self) -> i64 {
        self::delegate_to_executor!(self, id)
    }

    pub fn name(&self) -> &str {
        self::delegate_to_executor!(self, name)
    }

    pub fn description(&self) -> &str {
        self.name()
    }

    pub fn stats(&self) -> &ExecutorStats {
        self::delegate_to_executor!(self, stats)
    }

    pub fn stats_mut(&mut self) -> &mut ExecutorStats {
        self::delegate_to_executor_mut!(self, stats_mut)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for ExecutorEnum<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self::delegate_to_executor_mut!(self, execute)
    }

    fn open(&mut self) -> DBResult<()> {
        self::delegate_to_executor_mut!(self, open)
    }

    fn close(&mut self) -> DBResult<()> {
        self::delegate_to_executor_mut!(self, close)
    }

    fn is_open(&self) -> bool {
        self::delegate_to_executor!(self, is_open)
    }

    fn id(&self) -> i64 {
        self.id()
    }

    fn name(&self) -> &str {
        self.name()
    }

    fn description(&self) -> &str {
        self.name()
    }

    fn stats(&self) -> &ExecutorStats {
        self.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for ExecutorEnum<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        match self {
            ExecutorEnum::Filter(exec) => exec.set_input(input),
            ExecutorEnum::Project(exec) => exec.set_input(input),
            ExecutorEnum::Limit(exec) => exec.set_input(input),
            ExecutorEnum::Sort(exec) => exec.set_input(input),
            ExecutorEnum::TopN(exec) => exec.set_input(input),
            ExecutorEnum::Sample(exec) => exec.set_input(input),
            ExecutorEnum::Dedup(exec) => exec.set_input(input),
            ExecutorEnum::Expand(exec) => exec.set_input(input),
            ExecutorEnum::ExpandAll(exec) => exec.set_input(input),
            ExecutorEnum::Traverse(exec) => exec.set_input(input),
            ExecutorEnum::ShortestPath(exec) => exec.set_input(input),
            ExecutorEnum::Aggregate(exec) => exec.set_input(input),
            ExecutorEnum::GroupBy(exec) => exec.set_input(input),
            ExecutorEnum::Having(exec) => exec.set_input(input),
            ExecutorEnum::Remove(exec) => exec.set_input(input),
            ExecutorEnum::Materialize(exec) => exec.set_input(input),
            ExecutorEnum::Unwind(exec) => exec.set_input(input),
            ExecutorEnum::PipeDelete(exec) => exec.set_input(input),
            _ => {}
        }
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        match self {
            ExecutorEnum::Filter(exec) => exec.get_input(),
            ExecutorEnum::Project(exec) => exec.get_input(),
            ExecutorEnum::Limit(exec) => exec.get_input(),
            ExecutorEnum::Sort(exec) => exec.get_input(),
            ExecutorEnum::TopN(exec) => exec.get_input(),
            ExecutorEnum::Sample(exec) => exec.get_input(),
            ExecutorEnum::Dedup(exec) => exec.get_input(),
            ExecutorEnum::Expand(exec) => exec.get_input(),
            ExecutorEnum::ExpandAll(exec) => exec.get_input(),
            ExecutorEnum::Traverse(exec) => exec.get_input(),
            ExecutorEnum::ShortestPath(exec) => exec.get_input(),
            ExecutorEnum::MultiShortestPath(exec) => exec.get_input(),
            ExecutorEnum::Aggregate(exec) => exec.get_input(),
            ExecutorEnum::GroupBy(exec) => exec.get_input(),
            ExecutorEnum::Having(exec) => exec.get_input(),
            ExecutorEnum::Remove(exec) => exec.get_input(),
            ExecutorEnum::Materialize(exec) => exec.get_input(),
            ExecutorEnum::Unwind(exec) => exec.get_input(),
            ExecutorEnum::PipeDelete(exec) => exec.get_input(),
            _ => None,
        }
    }
}

pub trait ChainableExecutor<S: StorageClient + Send + 'static>:
    Executor<S> + InputExecutor<S>
{
    fn into_executor_enum(self) -> ExecutorEnum<S>
    where
        Self: Sized + 'static;
}

impl<S: StorageClient + Send + 'static> ChainableExecutor<S> for ExecutorEnum<S> {
    fn into_executor_enum(self) -> ExecutorEnum<S> {
        self
    }
}

use crate::query::core::{NodeCategory, NodeType};

impl<S: StorageClient + Send + 'static> NodeType for ExecutorEnum<S> {
    fn node_type_id(&self) -> &'static str {
        match self {
            ExecutorEnum::Start(_) => "start",
            ExecutorEnum::Base(_) => "base",
            ExecutorEnum::GetVertices(_) => "get_vertices",
            ExecutorEnum::GetEdges(_) => "get_edges",
            ExecutorEnum::GetNeighbors(_) => "get_neighbors",
            ExecutorEnum::GetProp(_) => "get_prop",
            ExecutorEnum::AllPaths(_) => "all_paths",
            ExecutorEnum::Expand(_) => "expand",
            ExecutorEnum::ExpandAll(_) => "expand_all",
            ExecutorEnum::Traverse(_) => "traverse",
            ExecutorEnum::BiExpand(_) => "bi_expand",
            ExecutorEnum::BiTraverse(_) => "bi_traverse",
            ExecutorEnum::ShortestPath(_) => "shortest_path",
            ExecutorEnum::MultiShortestPath(_) => "multi_shortest_path",
            ExecutorEnum::InnerJoin(_) => "inner_join",
            ExecutorEnum::HashInnerJoin(_) => "hash_inner_join",
            ExecutorEnum::LeftJoin(_) => "left_join",
            ExecutorEnum::HashLeftJoin(_) => "hash_left_join",
            ExecutorEnum::FullOuterJoin(_) => "full_outer_join",
            ExecutorEnum::CrossJoin(_) => "cross_join",
            ExecutorEnum::Union(_) => "union",
            ExecutorEnum::UnionAll(_) => "union_all",
            ExecutorEnum::Minus(_) => "minus",
            ExecutorEnum::Intersect(_) => "intersect",
            ExecutorEnum::Filter(_) => "filter",
            ExecutorEnum::Project(_) => "project",
            ExecutorEnum::Limit(_) => "limit",
            ExecutorEnum::Sort(_) => "sort",
            ExecutorEnum::TopN(_) => "topn",
            ExecutorEnum::Sample(_) => "sample",
            ExecutorEnum::Aggregate(_) => "aggregate",
            ExecutorEnum::GroupBy(_) => "group_by",
            ExecutorEnum::Having(_) => "having",
            ExecutorEnum::Dedup(_) => "dedup",
            ExecutorEnum::Unwind(_) => "unwind",
            ExecutorEnum::Assign(_) => "assign",
            ExecutorEnum::Materialize(_) => "materialize",
            ExecutorEnum::AppendVertices(_) => "append_vertices",
            ExecutorEnum::RollUpApply(_) => "rollup_apply",
            ExecutorEnum::PatternApply(_) => "pattern_apply",
            ExecutorEnum::Remove(_) => "remove",
            ExecutorEnum::InsertVertices(_) => "insert_vertices",
            ExecutorEnum::InsertEdges(_) => "insert_edges",
            ExecutorEnum::Loop(_) => "loop",
            ExecutorEnum::ForLoop(_) => "for_loop",
            ExecutorEnum::WhileLoop(_) => "while_loop",
            ExecutorEnum::Select(_) => "select",
            ExecutorEnum::ScanEdges(_) => "scan_edges",
            ExecutorEnum::ScanVertices(_) => "scan_vertices",
            ExecutorEnum::IndexScan(_) => "index_scan",
            ExecutorEnum::Argument(_) => "argument",
            ExecutorEnum::PassThrough(_) => "pass_through",
            ExecutorEnum::DataCollect(_) => "data_collect",
            ExecutorEnum::BFSShortest(_) => "bfs_shortest",
            // Management Executors (parameterized)
            ExecutorEnum::SpaceManage(e) => e.node_type_id(),
            ExecutorEnum::TagManage(e) => e.node_type_id(),
            ExecutorEnum::EdgeManage(e) => e.node_type_id(),
            ExecutorEnum::IndexManage(e) => e.node_type_id(),
            ExecutorEnum::UserManage(e) => e.node_type_id(),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(e) => e.node_type_id(),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(e) => e.node_type_id(),
            // Statistics
            ExecutorEnum::ShowStats(_) => "show_stats",
            ExecutorEnum::Analyze(_) => "analyze",
            ExecutorEnum::Delete(_) => "delete",
            ExecutorEnum::PipeDelete(_) => "pipe_delete",
            ExecutorEnum::Update(_) => "update",
            // Full-text Search Executors (data access)
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => "fulltext_search",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => "fulltext_lookup",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => "match_fulltext",
            // Vector Search Executors (data access)
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorSearch(_) => "vector_search",
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorLookup(_) => "vector_lookup",
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorMatch(_) => "vector_match",
        }
    }

    fn node_type_name(&self) -> &'static str {
        match self {
            ExecutorEnum::Start(_) => "Start",
            ExecutorEnum::Base(_) => "Base",
            ExecutorEnum::GetVertices(_) => "Get Vertices",
            ExecutorEnum::GetEdges(_) => "Get Edges",
            ExecutorEnum::GetNeighbors(_) => "Get Neighbors",
            ExecutorEnum::GetProp(_) => "Get Properties",
            ExecutorEnum::AllPaths(_) => "All Paths",
            ExecutorEnum::Expand(_) => "Expand",
            ExecutorEnum::ExpandAll(_) => "Expand All",
            ExecutorEnum::Traverse(_) => "Traverse",
            ExecutorEnum::BiExpand(_) => "BiExpand",
            ExecutorEnum::BiTraverse(_) => "BiTraverse",
            ExecutorEnum::ShortestPath(_) => "Shortest Path",
            ExecutorEnum::MultiShortestPath(_) => "Multi Shortest Path",
            ExecutorEnum::InnerJoin(_) => "Inner Join",
            ExecutorEnum::HashInnerJoin(_) => "Hash Inner Join",
            ExecutorEnum::LeftJoin(_) => "Left Join",
            ExecutorEnum::HashLeftJoin(_) => "Hash Left Join",
            ExecutorEnum::FullOuterJoin(_) => "Full Outer Join",
            ExecutorEnum::CrossJoin(_) => "Cross Join",
            ExecutorEnum::Union(_) => "Union",
            ExecutorEnum::UnionAll(_) => "Union All",
            ExecutorEnum::Minus(_) => "Minus",
            ExecutorEnum::Intersect(_) => "Intersect",
            ExecutorEnum::Filter(_) => "Filter",
            ExecutorEnum::Project(_) => "Project",
            ExecutorEnum::Limit(_) => "Limit",
            ExecutorEnum::Sort(_) => "Sort",
            ExecutorEnum::TopN(_) => "Top N",
            ExecutorEnum::Sample(_) => "Sample",
            ExecutorEnum::Aggregate(_) => "Aggregate",
            ExecutorEnum::GroupBy(_) => "Group By",
            ExecutorEnum::Having(_) => "Having",
            ExecutorEnum::Dedup(_) => "Dedup",
            ExecutorEnum::Unwind(_) => "Unwind",
            ExecutorEnum::Assign(_) => "Assign",
            ExecutorEnum::Materialize(_) => "Materialize",
            ExecutorEnum::AppendVertices(_) => "Append Vertices",
            ExecutorEnum::RollUpApply(_) => "RollUp Apply",
            ExecutorEnum::PatternApply(_) => "Pattern Apply",
            ExecutorEnum::Remove(_) => "Remove",
            ExecutorEnum::InsertVertices(_) => "Insert Vertices",
            ExecutorEnum::InsertEdges(_) => "Insert Edges",
            ExecutorEnum::Loop(_) => "Loop",
            ExecutorEnum::ForLoop(_) => "For Loop",
            ExecutorEnum::WhileLoop(_) => "While Loop",
            ExecutorEnum::Select(_) => "Select",
            ExecutorEnum::ScanEdges(_) => "Scan Edges",
            ExecutorEnum::ScanVertices(_) => "Scan Vertices",
            ExecutorEnum::IndexScan(_) => "Index Scan",
            ExecutorEnum::Argument(_) => "Argument",
            ExecutorEnum::PassThrough(_) => "Pass Through",
            ExecutorEnum::DataCollect(_) => "Data Collect",
            ExecutorEnum::BFSShortest(_) => "BFS Shortest",
            // Management Executors (parameterized)
            ExecutorEnum::SpaceManage(e) => e.node_type_name(),
            ExecutorEnum::TagManage(e) => e.node_type_name(),
            ExecutorEnum::EdgeManage(e) => e.node_type_name(),
            ExecutorEnum::IndexManage(e) => e.node_type_name(),
            ExecutorEnum::UserManage(e) => e.node_type_name(),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(e) => e.node_type_name(),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(e) => e.node_type_name(),
            // Statistics
            ExecutorEnum::ShowStats(_) => "Show Stats",
            ExecutorEnum::Analyze(_) => "Analyze",
            ExecutorEnum::Delete(_) => "Delete",
            ExecutorEnum::PipeDelete(_) => "Pipe Delete",
            ExecutorEnum::Update(_) => "Update",
            // Full-text Search Executors (data access)
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => "Fulltext Search",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => "Fulltext Lookup",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => "Match Fulltext",
            // Vector Search Executors (data access)
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorSearch(_) => "Vector Search",
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorLookup(_) => "Vector Lookup",
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorMatch(_) => "Vector Match",
        }
    }

    fn category(&self) -> NodeCategory {
        match self {
            ExecutorEnum::Start(_) => NodeCategory::Other,
            ExecutorEnum::Base(_) => NodeCategory::Other,
            ExecutorEnum::GetVertices(_) => NodeCategory::Scan,
            ExecutorEnum::GetEdges(_) => NodeCategory::Scan,
            ExecutorEnum::GetNeighbors(_) => NodeCategory::Scan,
            ExecutorEnum::GetProp(_) => NodeCategory::Scan,
            ExecutorEnum::AllPaths(_) => NodeCategory::Path,
            ExecutorEnum::Expand(_) => NodeCategory::Traversal,
            ExecutorEnum::ExpandAll(_) => NodeCategory::Traversal,
            ExecutorEnum::Traverse(_) => NodeCategory::Traversal,
            ExecutorEnum::BiExpand(_) => NodeCategory::Traversal,
            ExecutorEnum::BiTraverse(_) => NodeCategory::Traversal,
            ExecutorEnum::ShortestPath(_) => NodeCategory::Path,
            ExecutorEnum::MultiShortestPath(_) => NodeCategory::Path,
            ExecutorEnum::InnerJoin(_) => NodeCategory::Join,
            ExecutorEnum::HashInnerJoin(_) => NodeCategory::Join,
            ExecutorEnum::LeftJoin(_) => NodeCategory::Join,
            ExecutorEnum::HashLeftJoin(_) => NodeCategory::Join,
            ExecutorEnum::FullOuterJoin(_) => NodeCategory::Join,
            ExecutorEnum::CrossJoin(_) => NodeCategory::Join,
            ExecutorEnum::Union(_) => NodeCategory::SetOp,
            ExecutorEnum::UnionAll(_) => NodeCategory::SetOp,
            ExecutorEnum::Minus(_) => NodeCategory::SetOp,
            ExecutorEnum::Intersect(_) => NodeCategory::SetOp,
            ExecutorEnum::Filter(_) => NodeCategory::Filter,
            ExecutorEnum::Project(_) => NodeCategory::Project,
            ExecutorEnum::Limit(_) => NodeCategory::Other,
            ExecutorEnum::Sort(_) => NodeCategory::Sort,
            ExecutorEnum::TopN(_) => NodeCategory::Other,
            ExecutorEnum::Sample(_) => NodeCategory::Other,
            ExecutorEnum::Aggregate(_) => NodeCategory::Aggregate,
            ExecutorEnum::GroupBy(_) => NodeCategory::Aggregate,
            ExecutorEnum::Having(_) => NodeCategory::Filter,
            ExecutorEnum::Dedup(_) => NodeCategory::Other,
            ExecutorEnum::Unwind(_) => NodeCategory::Other,
            ExecutorEnum::Assign(_) => NodeCategory::Other,
            ExecutorEnum::Materialize(_) => NodeCategory::Other,
            ExecutorEnum::AppendVertices(_) => NodeCategory::Traversal,
            ExecutorEnum::RollUpApply(_) => NodeCategory::Other,
            ExecutorEnum::PatternApply(_) => NodeCategory::Other,
            ExecutorEnum::Remove(_) => NodeCategory::Other,
            ExecutorEnum::InsertVertices(_) => NodeCategory::Other,
            ExecutorEnum::InsertEdges(_) => NodeCategory::Other,
            ExecutorEnum::Loop(_) => NodeCategory::Control,
            ExecutorEnum::ForLoop(_) => NodeCategory::Control,
            ExecutorEnum::WhileLoop(_) => NodeCategory::Control,
            ExecutorEnum::Select(_) => NodeCategory::Control,
            ExecutorEnum::ScanEdges(_) => NodeCategory::Scan,
            ExecutorEnum::ScanVertices(_) => NodeCategory::Scan,
            ExecutorEnum::IndexScan(_) => NodeCategory::Scan,
            ExecutorEnum::Argument(_) => NodeCategory::Other,
            ExecutorEnum::PassThrough(_) => NodeCategory::Other,
            ExecutorEnum::DataCollect(_) => NodeCategory::DataCollect,
            ExecutorEnum::BFSShortest(_) => NodeCategory::Path,
            // Management Executors (parameterized)
            ExecutorEnum::SpaceManage(_) => NodeCategory::Admin,
            ExecutorEnum::TagManage(_) => NodeCategory::Admin,
            ExecutorEnum::EdgeManage(_) => NodeCategory::Admin,
            ExecutorEnum::IndexManage(_) => NodeCategory::Admin,
            ExecutorEnum::UserManage(_) => NodeCategory::Admin,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(_) => NodeCategory::Admin,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(_) => NodeCategory::Admin,
            // Statistics
            ExecutorEnum::ShowStats(_) => NodeCategory::Admin,
            ExecutorEnum::Analyze(_) => NodeCategory::Admin,
            ExecutorEnum::Delete(_) => NodeCategory::Admin,
            ExecutorEnum::PipeDelete(_) => NodeCategory::Admin,
            ExecutorEnum::Update(_) => NodeCategory::Admin,
            // Full-text Search Executors (data access)
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => NodeCategory::Scan,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => NodeCategory::Scan,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => NodeCategory::Scan,
            // Vector Search Executors (data access)
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorSearch(_) => NodeCategory::Scan,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorLookup(_) => NodeCategory::Scan,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorMatch(_) => NodeCategory::Scan,
        }
    }
}

/// Internal macro module – used to simplify the method delegation in ExecutorEnum
mod macros {
    /// Immutable methods that are delegated to internal executors
    macro_rules! delegate_to_executor {
        ($self:expr, $method:ident) => {
            match $self {
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::AllPaths(exec) => exec.$method(),
                ExecutorEnum::Expand(exec) => exec.$method(),
                ExecutorEnum::ExpandAll(exec) => exec.$method(),
                ExecutorEnum::Traverse(exec) => exec.$method(),
                ExecutorEnum::BiExpand(exec) => exec.$method(),
                ExecutorEnum::BiTraverse(exec) => exec.$method(),
                ExecutorEnum::ShortestPath(exec) => exec.$method(),
                ExecutorEnum::MultiShortestPath(exec) => exec.$method(),
                ExecutorEnum::InnerJoin(exec) => exec.$method(),
                ExecutorEnum::HashInnerJoin(exec) => exec.$method(),
                ExecutorEnum::LeftJoin(exec) => exec.$method(),
                ExecutorEnum::HashLeftJoin(exec) => exec.$method(),
                ExecutorEnum::FullOuterJoin(exec) => exec.$method(),
                ExecutorEnum::CrossJoin(exec) => exec.$method(),
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),
                ExecutorEnum::Limit(exec) => exec.$method(),
                ExecutorEnum::Sort(exec) => exec.$method(),
                ExecutorEnum::TopN(exec) => exec.$method(),
                ExecutorEnum::Sample(exec) => exec.$method(),
                ExecutorEnum::Aggregate(exec) => exec.$method(),
                ExecutorEnum::GroupBy(exec) => exec.$method(),
                ExecutorEnum::Having(exec) => exec.$method(),
                ExecutorEnum::Dedup(exec) => exec.$method(),
                ExecutorEnum::Unwind(exec) => exec.$method(),
                ExecutorEnum::Assign(exec) => exec.$method(),
                ExecutorEnum::Materialize(exec) => exec.$method(),
                ExecutorEnum::AppendVertices(exec) => exec.$method(),
                ExecutorEnum::RollUpApply(exec) => exec.$method(),
                ExecutorEnum::PatternApply(exec) => exec.$method(),
                ExecutorEnum::Remove(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),
                ExecutorEnum::BFSShortest(exec) => exec.$method(),
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorManage(exec) => exec.$method(),
                // Statistics
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),
                // Full-text Search Executors (data access)
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),
                // Vector Search Executors (data access)
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        };
    }

    /// A variable method that is delegated to an internal executor
    macro_rules! delegate_to_executor_mut {
        ($self:expr, $method:ident) => {
            match $self {
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::AllPaths(exec) => exec.$method(),
                ExecutorEnum::Expand(exec) => exec.$method(),
                ExecutorEnum::ExpandAll(exec) => exec.$method(),
                ExecutorEnum::Traverse(exec) => exec.$method(),
                ExecutorEnum::BiExpand(exec) => exec.$method(),
                ExecutorEnum::BiTraverse(exec) => exec.$method(),
                ExecutorEnum::ShortestPath(exec) => exec.$method(),
                ExecutorEnum::MultiShortestPath(exec) => exec.$method(),
                ExecutorEnum::InnerJoin(exec) => exec.$method(),
                ExecutorEnum::HashInnerJoin(exec) => exec.$method(),
                ExecutorEnum::LeftJoin(exec) => exec.$method(),
                ExecutorEnum::HashLeftJoin(exec) => exec.$method(),
                ExecutorEnum::FullOuterJoin(exec) => exec.$method(),
                ExecutorEnum::CrossJoin(exec) => exec.$method(),
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),
                ExecutorEnum::Limit(exec) => exec.$method(),
                ExecutorEnum::Sort(exec) => exec.$method(),
                ExecutorEnum::TopN(exec) => exec.$method(),
                ExecutorEnum::Sample(exec) => exec.$method(),
                ExecutorEnum::Aggregate(exec) => exec.$method(),
                ExecutorEnum::GroupBy(exec) => exec.$method(),
                ExecutorEnum::Having(exec) => exec.$method(),
                ExecutorEnum::Dedup(exec) => exec.$method(),
                ExecutorEnum::Unwind(exec) => exec.$method(),
                ExecutorEnum::Assign(exec) => exec.$method(),
                ExecutorEnum::Materialize(exec) => exec.$method(),
                ExecutorEnum::AppendVertices(exec) => exec.$method(),
                ExecutorEnum::RollUpApply(exec) => exec.$method(),
                ExecutorEnum::PatternApply(exec) => exec.$method(),
                ExecutorEnum::Remove(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),
                ExecutorEnum::BFSShortest(exec) => exec.$method(),
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorManage(exec) => exec.$method(),
                // Statistics
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),
                // Full-text Search Executors (data access)
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),
                // Vector Search Executors (data access)
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        };
    }

    pub(crate) use delegate_to_executor;
    pub(crate) use delegate_to_executor_mut;
}

use macros::{delegate_to_executor, delegate_to_executor_mut};
