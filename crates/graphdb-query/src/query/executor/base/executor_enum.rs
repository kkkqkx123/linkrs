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
    WindowExecutor,
};
use crate::query::executor::result_processing::transformations::{
    ApplyExecutor, AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor,
    RollUpApplyExecutor, UnwindExecutor,
};
use crate::query::executor::result_processing::{
    DedupExecutor, LimitExecutor, SampleExecutor, SortExecutor, TopNExecutor,
};
use crate::query::executor::utils::{ArgumentExecutor, DataCollectExecutor, PassThroughExecutor};

// ========== Sub-enum: Join Executors ==========
pub enum JoinExecutor<S: StorageClient + Send + 'static> {
    Inner(InnerJoinExecutor<S>),
    HashInner(HashInnerJoinExecutor<S>),
    Left(LeftJoinExecutor<S>),
    HashLeft(HashLeftJoinExecutor<S>),
    FullOuter(FullOuterJoinExecutor<S>),
    Cross(CrossJoinExecutor<S>),
}

// ========== Sub-enum: Graph Operation Executors ==========
pub enum GraphOperationExecutor<S: StorageClient + Send + 'static> {
    AllPaths(AllPathsExecutor<S>),
    Expand(ExpandExecutor<S>),
    ExpandAll(ExpandAllExecutor<S>),
    Traverse(TraverseExecutor<S>),
    BiExpand(ExpandExecutor<S>),
    BiTraverse(ExpandExecutor<S>),
    ShortestPath(ShortestPathExecutor<S>),
    MultiShortestPath(MultiShortestPathExecutor<S>),
    BFSShortest(BFSShortestExecutor<S>),
}

// ========== Sub-enum: Result Processing Executors ==========
pub enum ResultProcessingExecutor<S: StorageClient + Send + 'static> {
    Limit(LimitExecutor<S>),
    Sort(SortExecutor<S>),
    TopN(TopNExecutor<S>),
    Sample(SampleExecutor<S>),
    Aggregate(AggregateExecutor<S>),
    GroupBy(GroupByExecutor<S>),
    Having(HavingExecutor<S>),
    Window(WindowExecutor<S>),
    Dedup(DedupExecutor<S>),
    Unwind(UnwindExecutor<S>),
    Assign(AssignExecutor<S>),
    Materialize(MaterializeExecutor<S>),
    AppendVertices(AppendVerticesExecutor<S>),
    RollUpApply(RollUpApplyExecutor<S>),
    PatternApply(PatternApplyExecutor<S>),
    Apply(ApplyExecutor<S>),
    Remove(RemoveExecutor<S>),
}

/// Executor enumeration
///
/// Include all possible types of executors, and implement polymorphism using static distribution.
/// Uses sub-enums (JoinExecutor, GraphOperationExecutor, ResultProcessingExecutor) to reduce
/// the total number of variants while maintaining type safety.
pub enum ExecutorEnum<S: StorageClient + Send + 'static> {
    // ========== Basic Executors ==========
    Start(StartExecutor<S>),
    Base(BaseExecutor<S>),

    // ========== Data Access Executors ==========
    GetVertices(GetVerticesExecutor<S>),
    GetEdges(GetEdgesExecutor<S>),
    GetNeighbors(GetNeighborsExecutor<S>),
    GetProp(GetPropExecutor<S>),
    ScanEdges(ScanEdgesExecutor<S>),
    ScanVertices(ScanVerticesExecutor<S>),
    IndexScan(IndexScanExecutor<S>),

    // ========== Join Executors (6 variants → 1) ==========
    Join(JoinExecutor<S>),

    // ========== Graph Operation Executors (9 variants → 1) ==========
    GraphOperation(GraphOperationExecutor<S>),

    // ========== Set Operations ==========
    Union(UnionExecutor<S>),
    UnionAll(UnionAllExecutor<S>),
    Minus(MinusExecutor<S>),
    Intersect(IntersectExecutor<S>),

    // ========== Basic Relational Operators ==========
    Filter(FilterExecutor<S>),
    Project(ProjectExecutor<S>),

    // ========== Result Processing Executors (17 variants → 1) ==========
    ResultProcessing(ResultProcessingExecutor<S>),

    // ========== Data Modification Executors ==========
    InsertVertices(InsertExecutor<S>),
    InsertEdges(InsertExecutor<S>),
    Update(UpdateExecutor<S>),
    Delete(DeleteExecutor<S>),
    PipeDelete(PipeDeleteExecutor<S>),

    // ========== Control Flow Executors ==========
    Loop(LoopExecutor<S>),
    ForLoop(ForLoopExecutor<S>),
    WhileLoop(WhileLoopExecutor<S>),
    Select(SelectExecutor<S>),

    // ========== Utility Executors ==========
    Argument(ArgumentExecutor<S>),
    PassThrough(PassThroughExecutor<S>),
    DataCollect(DataCollectExecutor<S>),

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

    // ========== Statistics Executors ==========
    ShowStats(crate::query::executor::admin::query_management::show_stats::ShowStatsExecutor<S>),
    Analyze(AnalyzeExecutor<S>),

    // ========== Full-text Search Executors (data access) ==========
    #[cfg(feature = "fulltext-search")]
    FulltextSearch(FulltextSearchExecutor<S>),
    #[cfg(feature = "fulltext-search")]
    FulltextLookup(FulltextScanExecutor<S>),
    #[cfg(feature = "fulltext-search")]
    MatchFulltext(MatchFulltextExecutor<S>),

    // ========== Vector Search Executors (data access) ==========
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
            // Basic Executors
            ExecutorEnum::Start(exec) => ("Start", exec.name()),
            ExecutorEnum::Base(exec) => ("Base", exec.name()),

            // Data Access Executors
            ExecutorEnum::GetVertices(exec) => ("GetVertices", exec.name()),
            ExecutorEnum::GetEdges(exec) => ("GetEdges", exec.name()),
            ExecutorEnum::GetNeighbors(exec) => ("GetNeighbors", exec.name()),
            ExecutorEnum::GetProp(exec) => ("GetProp", exec.name()),
            ExecutorEnum::ScanEdges(exec) => ("ScanEdges", exec.name()),
            ExecutorEnum::ScanVertices(exec) => ("ScanVertices", exec.name()),
            ExecutorEnum::IndexScan(exec) => ("IndexScan", exec.name()),

            // Join Executors
            ExecutorEnum::Join(exec) => match exec {
                JoinExecutor::Inner(e) => ("Join::Inner", e.name()),
                JoinExecutor::HashInner(e) => ("Join::HashInner", e.name()),
                JoinExecutor::Left(e) => ("Join::Left", e.name()),
                JoinExecutor::HashLeft(e) => ("Join::HashLeft", e.name()),
                JoinExecutor::FullOuter(e) => ("Join::FullOuter", e.name()),
                JoinExecutor::Cross(e) => ("Join::Cross", e.name()),
            },

            // Graph Operation Executors
            ExecutorEnum::GraphOperation(exec) => match exec {
                GraphOperationExecutor::AllPaths(e) => ("GraphOperation::AllPaths", e.name()),
                GraphOperationExecutor::Expand(e) => ("GraphOperation::Expand", e.name()),
                GraphOperationExecutor::ExpandAll(e) => ("GraphOperation::ExpandAll", e.name()),
                GraphOperationExecutor::Traverse(e) => ("GraphOperation::Traverse", e.name()),
                GraphOperationExecutor::BiExpand(e) => ("GraphOperation::BiExpand", e.name()),
                GraphOperationExecutor::BiTraverse(e) => ("GraphOperation::BiTraverse", e.name()),
                GraphOperationExecutor::ShortestPath(e) => ("GraphOperation::ShortestPath", e.name()),
                GraphOperationExecutor::MultiShortestPath(e) => ("GraphOperation::MultiShortestPath", e.name()),
                GraphOperationExecutor::BFSShortest(e) => ("GraphOperation::BFSShortest", e.name()),
            },

            // Set Operations
            ExecutorEnum::Union(exec) => ("Union", exec.name()),
            ExecutorEnum::UnionAll(exec) => ("UnionAll", exec.name()),
            ExecutorEnum::Minus(exec) => ("Minus", exec.name()),
            ExecutorEnum::Intersect(exec) => ("Intersect", exec.name()),

            // Basic Relational Operators
            ExecutorEnum::Filter(exec) => ("Filter", exec.name()),
            ExecutorEnum::Project(exec) => ("Project", exec.name()),

            // Result Processing Executors
            ExecutorEnum::ResultProcessing(exec) => match exec {
                ResultProcessingExecutor::Limit(e) => ("ResultProcessing::Limit", e.name()),
                ResultProcessingExecutor::Sort(e) => ("ResultProcessing::Sort", e.name()),
                ResultProcessingExecutor::TopN(e) => ("ResultProcessing::TopN", e.name()),
                ResultProcessingExecutor::Sample(e) => ("ResultProcessing::Sample", e.name()),
                ResultProcessingExecutor::Aggregate(e) => ("ResultProcessing::Aggregate", e.name()),
                ResultProcessingExecutor::GroupBy(e) => ("ResultProcessing::GroupBy", e.name()),
                ResultProcessingExecutor::Having(e) => ("ResultProcessing::Having", e.name()),
                ResultProcessingExecutor::Window(e) => ("ResultProcessing::Window", e.name()),
                ResultProcessingExecutor::Dedup(e) => ("ResultProcessing::Dedup", e.name()),
                ResultProcessingExecutor::Unwind(e) => ("ResultProcessing::Unwind", e.name()),
                ResultProcessingExecutor::Assign(e) => ("ResultProcessing::Assign", e.name()),
                ResultProcessingExecutor::Materialize(e) => ("ResultProcessing::Materialize", e.name()),
                ResultProcessingExecutor::AppendVertices(e) => ("ResultProcessing::AppendVertices", e.name()),
                ResultProcessingExecutor::RollUpApply(e) => ("ResultProcessing::RollUpApply", e.name()),
                ResultProcessingExecutor::PatternApply(e) => ("ResultProcessing::PatternApply", e.name()),
                ResultProcessingExecutor::Apply(e) => ("ResultProcessing::Apply", e.name()),
                ResultProcessingExecutor::Remove(e) => ("ResultProcessing::Remove", e.name()),
            },

            // Data Modification Executors
            ExecutorEnum::InsertVertices(exec) => ("InsertVertices", exec.name()),
            ExecutorEnum::InsertEdges(exec) => ("InsertEdges", exec.name()),
            ExecutorEnum::Update(exec) => ("Update", exec.name()),
            ExecutorEnum::Delete(exec) => ("Delete", exec.name()),
            ExecutorEnum::PipeDelete(exec) => ("PipeDelete", exec.name()),

            // Control Flow Executors
            ExecutorEnum::Loop(exec) => ("Loop", exec.name()),
            ExecutorEnum::ForLoop(exec) => ("ForLoop", exec.name()),
            ExecutorEnum::WhileLoop(exec) => ("WhileLoop", exec.name()),
            ExecutorEnum::Select(exec) => ("Select", exec.name()),

            // Utility Executors
            ExecutorEnum::Argument(exec) => ("Argument", exec.name()),
            ExecutorEnum::PassThrough(exec) => ("PassThrough", exec.name()),
            ExecutorEnum::DataCollect(exec) => ("DataCollect", exec.name()),

            // Management Executors
            ExecutorEnum::SpaceManage(exec) => ("SpaceManage", exec.name()),
            ExecutorEnum::TagManage(exec) => ("TagManage", exec.name()),
            ExecutorEnum::EdgeManage(exec) => ("EdgeManage", exec.name()),
            ExecutorEnum::IndexManage(exec) => ("IndexManage", exec.name()),
            ExecutorEnum::UserManage(exec) => ("UserManage", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(exec) => ("FulltextManage", exec.name()),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(exec) => ("VectorManage", exec.name()),

            // Statistics Executors
            ExecutorEnum::ShowStats(exec) => ("ShowStats", exec.name()),
            ExecutorEnum::Analyze(exec) => ("Analyze", exec.name()),

            // Full-text Search Executors
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(exec) => ("FulltextSearch", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(exec) => ("FulltextLookup", exec.name()),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(exec) => ("MatchFulltext", exec.name()),

            // Vector Search Executors
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
        self::macros::call_input_executor_set_input!(self, input)
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self::macros::call_input_executor_get_input!(self)
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
            // Basic Executors
            ExecutorEnum::Start(_) => "start",
            ExecutorEnum::Base(_) => "base",

            // Data Access Executors
            ExecutorEnum::GetVertices(_) => "get_vertices",
            ExecutorEnum::GetEdges(_) => "get_edges",
            ExecutorEnum::GetNeighbors(_) => "get_neighbors",
            ExecutorEnum::GetProp(_) => "get_prop",
            ExecutorEnum::ScanEdges(_) => "scan_edges",
            ExecutorEnum::ScanVertices(_) => "scan_vertices",
            ExecutorEnum::IndexScan(_) => "index_scan",

            // Join Executors
            ExecutorEnum::Join(exec) => match exec {
                JoinExecutor::Inner(_) => "inner_join",
                JoinExecutor::HashInner(_) => "hash_inner_join",
                JoinExecutor::Left(_) => "left_join",
                JoinExecutor::HashLeft(_) => "hash_left_join",
                JoinExecutor::FullOuter(_) => "full_outer_join",
                JoinExecutor::Cross(_) => "cross_join",
            },

            // Graph Operation Executors
            ExecutorEnum::GraphOperation(exec) => match exec {
                GraphOperationExecutor::AllPaths(_) => "all_paths",
                GraphOperationExecutor::Expand(_) => "expand",
                GraphOperationExecutor::ExpandAll(_) => "expand_all",
                GraphOperationExecutor::Traverse(_) => "traverse",
                GraphOperationExecutor::BiExpand(_) => "bi_expand",
                GraphOperationExecutor::BiTraverse(_) => "bi_traverse",
                GraphOperationExecutor::ShortestPath(_) => "shortest_path",
                GraphOperationExecutor::MultiShortestPath(_) => "multi_shortest_path",
                GraphOperationExecutor::BFSShortest(_) => "bfs_shortest",
            },

            // Set Operations
            ExecutorEnum::Union(_) => "union",
            ExecutorEnum::UnionAll(_) => "union_all",
            ExecutorEnum::Minus(_) => "minus",
            ExecutorEnum::Intersect(_) => "intersect",

            // Basic Relational Operators
            ExecutorEnum::Filter(_) => "filter",
            ExecutorEnum::Project(_) => "project",

            // Result Processing Executors
            ExecutorEnum::ResultProcessing(exec) => match exec {
                ResultProcessingExecutor::Limit(_) => "limit",
                ResultProcessingExecutor::Sort(_) => "sort",
                ResultProcessingExecutor::TopN(_) => "topn",
                ResultProcessingExecutor::Sample(_) => "sample",
                ResultProcessingExecutor::Aggregate(_) => "aggregate",
                ResultProcessingExecutor::GroupBy(_) => "group_by",
                ResultProcessingExecutor::Having(_) => "having",
                ResultProcessingExecutor::Window(_) => "window",
                ResultProcessingExecutor::Dedup(_) => "dedup",
                ResultProcessingExecutor::Unwind(_) => "unwind",
                ResultProcessingExecutor::Assign(_) => "assign",
                ResultProcessingExecutor::Materialize(_) => "materialize",
                ResultProcessingExecutor::AppendVertices(_) => "append_vertices",
                ResultProcessingExecutor::RollUpApply(_) => "rollup_apply",
                ResultProcessingExecutor::PatternApply(_) => "pattern_apply",
                ResultProcessingExecutor::Apply(_) => "apply",
                ResultProcessingExecutor::Remove(_) => "remove",
            },

            // Data Modification Executors
            ExecutorEnum::InsertVertices(_) => "insert_vertices",
            ExecutorEnum::InsertEdges(_) => "insert_edges",
            ExecutorEnum::Update(_) => "update",
            ExecutorEnum::Delete(_) => "delete",
            ExecutorEnum::PipeDelete(_) => "pipe_delete",

            // Control Flow Executors
            ExecutorEnum::Loop(_) => "loop",
            ExecutorEnum::ForLoop(_) => "for_loop",
            ExecutorEnum::WhileLoop(_) => "while_loop",
            ExecutorEnum::Select(_) => "select",

            // Utility Executors
            ExecutorEnum::Argument(_) => "argument",
            ExecutorEnum::PassThrough(_) => "pass_through",
            ExecutorEnum::DataCollect(_) => "data_collect",

            // Management Executors
            ExecutorEnum::SpaceManage(e) => e.node_type_id(),
            ExecutorEnum::TagManage(e) => e.node_type_id(),
            ExecutorEnum::EdgeManage(e) => e.node_type_id(),
            ExecutorEnum::IndexManage(e) => e.node_type_id(),
            ExecutorEnum::UserManage(e) => e.node_type_id(),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(e) => e.node_type_id(),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(e) => e.node_type_id(),

            // Statistics Executors
            ExecutorEnum::ShowStats(_) => "show_stats",
            ExecutorEnum::Analyze(_) => "analyze",

            // Full-text Search Executors
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => "fulltext_search",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => "fulltext_lookup",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => "match_fulltext",

            // Vector Search Executors
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
            // Basic Executors
            ExecutorEnum::Start(_) => "Start",
            ExecutorEnum::Base(_) => "Base",

            // Data Access Executors
            ExecutorEnum::GetVertices(_) => "Get Vertices",
            ExecutorEnum::GetEdges(_) => "Get Edges",
            ExecutorEnum::GetNeighbors(_) => "Get Neighbors",
            ExecutorEnum::GetProp(_) => "Get Properties",
            ExecutorEnum::ScanEdges(_) => "Scan Edges",
            ExecutorEnum::ScanVertices(_) => "Scan Vertices",
            ExecutorEnum::IndexScan(_) => "Index Scan",

            // Join Executors
            ExecutorEnum::Join(exec) => match exec {
                JoinExecutor::Inner(_) => "Inner Join",
                JoinExecutor::HashInner(_) => "Hash Inner Join",
                JoinExecutor::Left(_) => "Left Join",
                JoinExecutor::HashLeft(_) => "Hash Left Join",
                JoinExecutor::FullOuter(_) => "Full Outer Join",
                JoinExecutor::Cross(_) => "Cross Join",
            },

            // Graph Operation Executors
            ExecutorEnum::GraphOperation(exec) => match exec {
                GraphOperationExecutor::AllPaths(_) => "All Paths",
                GraphOperationExecutor::Expand(_) => "Expand",
                GraphOperationExecutor::ExpandAll(_) => "Expand All",
                GraphOperationExecutor::Traverse(_) => "Traverse",
                GraphOperationExecutor::BiExpand(_) => "BiExpand",
                GraphOperationExecutor::BiTraverse(_) => "BiTraverse",
                GraphOperationExecutor::ShortestPath(_) => "Shortest Path",
                GraphOperationExecutor::MultiShortestPath(_) => "Multi Shortest Path",
                GraphOperationExecutor::BFSShortest(_) => "BFS Shortest",
            },

            // Set Operations
            ExecutorEnum::Union(_) => "Union",
            ExecutorEnum::UnionAll(_) => "Union All",
            ExecutorEnum::Minus(_) => "Minus",
            ExecutorEnum::Intersect(_) => "Intersect",

            // Basic Relational Operators
            ExecutorEnum::Filter(_) => "Filter",
            ExecutorEnum::Project(_) => "Project",

            // Result Processing Executors
            ExecutorEnum::ResultProcessing(exec) => match exec {
                ResultProcessingExecutor::Limit(_) => "Limit",
                ResultProcessingExecutor::Sort(_) => "Sort",
                ResultProcessingExecutor::TopN(_) => "Top-N",
                ResultProcessingExecutor::Sample(_) => "Sample",
                ResultProcessingExecutor::Aggregate(_) => "Aggregate",
                ResultProcessingExecutor::GroupBy(_) => "Group By",
                ResultProcessingExecutor::Having(_) => "Having",
                ResultProcessingExecutor::Window(_) => "Window",
                ResultProcessingExecutor::Dedup(_) => "Dedup",
                ResultProcessingExecutor::Unwind(_) => "Unwind",
                ResultProcessingExecutor::Assign(_) => "Assign",
                ResultProcessingExecutor::Materialize(_) => "Materialize",
                ResultProcessingExecutor::AppendVertices(_) => "Append Vertices",
                ResultProcessingExecutor::RollUpApply(_) => "Roll Up Apply",
                ResultProcessingExecutor::PatternApply(_) => "Pattern Apply",
                ResultProcessingExecutor::Apply(_) => "Apply",
                ResultProcessingExecutor::Remove(_) => "Remove",
            },

            // Data Modification Executors
            ExecutorEnum::InsertVertices(_) => "Insert Vertices",
            ExecutorEnum::InsertEdges(_) => "Insert Edges",
            ExecutorEnum::Update(_) => "Update",
            ExecutorEnum::Delete(_) => "Delete",
            ExecutorEnum::PipeDelete(_) => "Pipe Delete",

            // Control Flow Executors
            ExecutorEnum::Loop(_) => "Loop",
            ExecutorEnum::ForLoop(_) => "For Loop",
            ExecutorEnum::WhileLoop(_) => "While Loop",
            ExecutorEnum::Select(_) => "Select",

            // Utility Executors
            ExecutorEnum::Argument(_) => "Argument",
            ExecutorEnum::PassThrough(_) => "Pass Through",
            ExecutorEnum::DataCollect(_) => "Data Collect",

            // Management Executors
            ExecutorEnum::SpaceManage(e) => e.node_type_name(),
            ExecutorEnum::TagManage(e) => e.node_type_name(),
            ExecutorEnum::EdgeManage(e) => e.node_type_name(),
            ExecutorEnum::IndexManage(e) => e.node_type_name(),
            ExecutorEnum::UserManage(e) => e.node_type_name(),
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(e) => e.node_type_name(),
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(e) => e.node_type_name(),

            // Statistics Executors
            ExecutorEnum::ShowStats(_) => "Show Stats",
            ExecutorEnum::Analyze(_) => "Analyze",

            // Full-text Search Executors
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => "Fulltext Search",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => "Fulltext Lookup",
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => "Match Fulltext",

            // Vector Search Executors
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
            // Basic Executors
            ExecutorEnum::Start(_) => NodeCategory::Other,
            ExecutorEnum::Base(_) => NodeCategory::Other,

            // Data Access Executors
            ExecutorEnum::GetVertices(_) => NodeCategory::Scan,
            ExecutorEnum::GetEdges(_) => NodeCategory::Scan,
            ExecutorEnum::GetNeighbors(_) => NodeCategory::Scan,
            ExecutorEnum::GetProp(_) => NodeCategory::Scan,
            ExecutorEnum::ScanEdges(_) => NodeCategory::Scan,
            ExecutorEnum::ScanVertices(_) => NodeCategory::Scan,
            ExecutorEnum::IndexScan(_) => NodeCategory::Scan,

            // Join Executors (all are Join operations)
            ExecutorEnum::Join(_) => NodeCategory::Join,

            // Graph Operation Executors
            ExecutorEnum::GraphOperation(exec) => match exec {
                GraphOperationExecutor::AllPaths(_) => NodeCategory::Path,
                GraphOperationExecutor::ShortestPath(_) => NodeCategory::Path,
                GraphOperationExecutor::MultiShortestPath(_) => NodeCategory::Path,
                GraphOperationExecutor::BFSShortest(_) => NodeCategory::Path,
                _ => NodeCategory::Traversal, // Expand, ExpandAll, Traverse, BiExpand, BiTraverse
            },

            // Set Operations
            ExecutorEnum::Union(_) => NodeCategory::SetOp,
            ExecutorEnum::UnionAll(_) => NodeCategory::SetOp,
            ExecutorEnum::Minus(_) => NodeCategory::SetOp,
            ExecutorEnum::Intersect(_) => NodeCategory::SetOp,

            // Basic Relational Operators
            ExecutorEnum::Filter(_) => NodeCategory::Filter,
            ExecutorEnum::Project(_) => NodeCategory::Project,

            // Result Processing Executors
            ExecutorEnum::ResultProcessing(exec) => match exec {
                ResultProcessingExecutor::Sort(_) => NodeCategory::Sort,
                ResultProcessingExecutor::Aggregate(_) => NodeCategory::Aggregate,
                ResultProcessingExecutor::GroupBy(_) => NodeCategory::Aggregate,
                ResultProcessingExecutor::Window(_) => NodeCategory::Aggregate,
                ResultProcessingExecutor::Having(_) => NodeCategory::Filter,
                _ => NodeCategory::Other, // Limit, TopN, Sample, Dedup, Unwind, Assign, Materialize, RollUpApply, PatternApply, Apply, Remove
            },

            // Data Modification Executors
            ExecutorEnum::InsertVertices(_) => NodeCategory::Other,
            ExecutorEnum::InsertEdges(_) => NodeCategory::Other,
            ExecutorEnum::Update(_) => NodeCategory::Admin,
            ExecutorEnum::Delete(_) => NodeCategory::Admin,
            ExecutorEnum::PipeDelete(_) => NodeCategory::Admin,

            // Control Flow Executors
            ExecutorEnum::Loop(_) => NodeCategory::Control,
            ExecutorEnum::ForLoop(_) => NodeCategory::Control,
            ExecutorEnum::WhileLoop(_) => NodeCategory::Control,
            ExecutorEnum::Select(_) => NodeCategory::Control,

            // Utility Executors
            ExecutorEnum::Argument(_) => NodeCategory::Other,
            ExecutorEnum::PassThrough(_) => NodeCategory::Other,
            ExecutorEnum::DataCollect(_) => NodeCategory::DataCollect,

            // Management Executors (all are Admin operations)
            ExecutorEnum::SpaceManage(_) => NodeCategory::Admin,
            ExecutorEnum::TagManage(_) => NodeCategory::Admin,
            ExecutorEnum::EdgeManage(_) => NodeCategory::Admin,
            ExecutorEnum::IndexManage(_) => NodeCategory::Admin,
            ExecutorEnum::UserManage(_) => NodeCategory::Admin,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextManage(_) => NodeCategory::Admin,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorManage(_) => NodeCategory::Admin,

            // Statistics Executors
            ExecutorEnum::ShowStats(_) => NodeCategory::Admin,
            ExecutorEnum::Analyze(_) => NodeCategory::Admin,

            // Full-text Search Executors
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextSearch(_) => NodeCategory::Scan,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::FulltextLookup(_) => NodeCategory::Scan,
            #[cfg(feature = "fulltext-search")]
            ExecutorEnum::MatchFulltext(_) => NodeCategory::Scan,

            // Vector Search Executors
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorSearch(_) => NodeCategory::Scan,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorLookup(_) => NodeCategory::Scan,
            #[cfg(feature = "qdrant")]
            ExecutorEnum::VectorMatch(_) => NodeCategory::Scan,
        }
    }
}

/// Internal macro module – used to simplify the method delegation in ExecutorEnum with sub-enums
mod macros {
    /// Immutable methods that are delegated to internal executors
    /// Supports recursive delegation for sub-enums (Join, GraphOperation, ResultProcessing)
    macro_rules! delegate_to_executor {
        ($self:expr, $method:ident) => {
            match $self {
                // Basic Executors
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),

                // Data Access Executors
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),

                // Join Executors - recursive delegation
                ExecutorEnum::Join(exec) => match exec {
                    JoinExecutor::Inner(e) => e.$method(),
                    JoinExecutor::HashInner(e) => e.$method(),
                    JoinExecutor::Left(e) => e.$method(),
                    JoinExecutor::HashLeft(e) => e.$method(),
                    JoinExecutor::FullOuter(e) => e.$method(),
                    JoinExecutor::Cross(e) => e.$method(),
                },

                // Graph Operation Executors - recursive delegation
                ExecutorEnum::GraphOperation(exec) => match exec {
                    GraphOperationExecutor::AllPaths(e) => e.$method(),
                    GraphOperationExecutor::Expand(e) => e.$method(),
                    GraphOperationExecutor::ExpandAll(e) => e.$method(),
                    GraphOperationExecutor::Traverse(e) => e.$method(),
                    GraphOperationExecutor::BiExpand(e) => e.$method(),
                    GraphOperationExecutor::BiTraverse(e) => e.$method(),
                    GraphOperationExecutor::ShortestPath(e) => e.$method(),
                    GraphOperationExecutor::MultiShortestPath(e) => e.$method(),
                    GraphOperationExecutor::BFSShortest(e) => e.$method(),
                },

                // Set Operations
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),

                // Basic Relational Operators
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),

                // Result Processing Executors - recursive delegation
                ExecutorEnum::ResultProcessing(exec) => match exec {
                    ResultProcessingExecutor::Limit(e) => e.$method(),
                    ResultProcessingExecutor::Sort(e) => e.$method(),
                    ResultProcessingExecutor::TopN(e) => e.$method(),
                    ResultProcessingExecutor::Sample(e) => e.$method(),
                    ResultProcessingExecutor::Aggregate(e) => e.$method(),
                    ResultProcessingExecutor::GroupBy(e) => e.$method(),
                    ResultProcessingExecutor::Having(e) => e.$method(),
                    ResultProcessingExecutor::Window(e) => e.$method(),
                    ResultProcessingExecutor::Dedup(e) => e.$method(),
                    ResultProcessingExecutor::Unwind(e) => e.$method(),
                    ResultProcessingExecutor::Assign(e) => e.$method(),
                    ResultProcessingExecutor::Materialize(e) => e.$method(),
                    ResultProcessingExecutor::AppendVertices(e) => e.$method(),
                    ResultProcessingExecutor::RollUpApply(e) => e.$method(),
                    ResultProcessingExecutor::PatternApply(e) => e.$method(),
                    ResultProcessingExecutor::Apply(e) => e.$method(),
                    ResultProcessingExecutor::Remove(e) => e.$method(),
                },

                // Data Modification Executors
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),

                // Control Flow Executors
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),

                // Utility Executors
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),

                // Management Executors
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorManage(exec) => exec.$method(),

                // Statistics Executors
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),

                // Full-text Search Executors
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),

                // Vector Search Executors
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        };
    }

    /// Mutable methods that are delegated to internal executors
    /// Supports recursive delegation for sub-enums (Join, GraphOperation, ResultProcessing)
    macro_rules! delegate_to_executor_mut {
        ($self:expr, $method:ident) => {
            match $self {
                // Basic Executors
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),

                // Data Access Executors
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),

                // Join Executors - recursive delegation
                ExecutorEnum::Join(exec) => match exec {
                    JoinExecutor::Inner(e) => e.$method(),
                    JoinExecutor::HashInner(e) => e.$method(),
                    JoinExecutor::Left(e) => e.$method(),
                    JoinExecutor::HashLeft(e) => e.$method(),
                    JoinExecutor::FullOuter(e) => e.$method(),
                    JoinExecutor::Cross(e) => e.$method(),
                },

                // Graph Operation Executors - recursive delegation
                ExecutorEnum::GraphOperation(exec) => match exec {
                    GraphOperationExecutor::AllPaths(e) => e.$method(),
                    GraphOperationExecutor::Expand(e) => e.$method(),
                    GraphOperationExecutor::ExpandAll(e) => e.$method(),
                    GraphOperationExecutor::Traverse(e) => e.$method(),
                    GraphOperationExecutor::BiExpand(e) => e.$method(),
                    GraphOperationExecutor::BiTraverse(e) => e.$method(),
                    GraphOperationExecutor::ShortestPath(e) => e.$method(),
                    GraphOperationExecutor::MultiShortestPath(e) => e.$method(),
                    GraphOperationExecutor::BFSShortest(e) => e.$method(),
                },

                // Set Operations
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),

                // Basic Relational Operators
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),

                // Result Processing Executors - recursive delegation
                ExecutorEnum::ResultProcessing(exec) => match exec {
                    ResultProcessingExecutor::Limit(e) => e.$method(),
                    ResultProcessingExecutor::Sort(e) => e.$method(),
                    ResultProcessingExecutor::TopN(e) => e.$method(),
                    ResultProcessingExecutor::Sample(e) => e.$method(),
                    ResultProcessingExecutor::Aggregate(e) => e.$method(),
                    ResultProcessingExecutor::GroupBy(e) => e.$method(),
                    ResultProcessingExecutor::Having(e) => e.$method(),
                    ResultProcessingExecutor::Window(e) => e.$method(),
                    ResultProcessingExecutor::Dedup(e) => e.$method(),
                    ResultProcessingExecutor::Unwind(e) => e.$method(),
                    ResultProcessingExecutor::Assign(e) => e.$method(),
                    ResultProcessingExecutor::Materialize(e) => e.$method(),
                    ResultProcessingExecutor::AppendVertices(e) => e.$method(),
                    ResultProcessingExecutor::RollUpApply(e) => e.$method(),
                    ResultProcessingExecutor::PatternApply(e) => e.$method(),
                    ResultProcessingExecutor::Apply(e) => e.$method(),
                    ResultProcessingExecutor::Remove(e) => e.$method(),
                },

                // Data Modification Executors
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),

                // Control Flow Executors
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),

                // Utility Executors
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),

                // Management Executors
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorManage(exec) => exec.$method(),

                // Statistics Executors
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),

                // Full-text Search Executors
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                #[cfg(feature = "fulltext-search")]
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),

                // Vector Search Executors
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                #[cfg(feature = "qdrant")]
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        };
    }

    /// Generate set_input match arms for InputExecutor trait
    /// Only applies to variants supporting input chaining
    macro_rules! call_input_executor_set_input {
        ($self:expr, $input:expr) => {
            match $self {
                // Basic Relational Operators
                ExecutorEnum::Filter(exec) => exec.set_input($input),
                ExecutorEnum::Project(exec) => exec.set_input($input),

                // Result Processing Executors
                ExecutorEnum::ResultProcessing(exec) => match exec {
                    ResultProcessingExecutor::Limit(e) => e.set_input($input),
                    ResultProcessingExecutor::Sort(e) => e.set_input($input),
                    ResultProcessingExecutor::TopN(e) => e.set_input($input),
                    ResultProcessingExecutor::Sample(e) => e.set_input($input),
                    ResultProcessingExecutor::Dedup(e) => e.set_input($input),
                    ResultProcessingExecutor::Aggregate(e) => e.set_input($input),
                    ResultProcessingExecutor::GroupBy(e) => e.set_input($input),
                    ResultProcessingExecutor::Having(e) => e.set_input($input),
                    ResultProcessingExecutor::Window(e) => e.set_input($input),
                    ResultProcessingExecutor::Materialize(e) => e.set_input($input),
                    ResultProcessingExecutor::Unwind(e) => e.set_input($input),
                    ResultProcessingExecutor::Remove(e) => e.set_input($input),
                    // Non-input supporting executors
                    ResultProcessingExecutor::Assign(_) => {},
                    ResultProcessingExecutor::AppendVertices(_) => {},
                    ResultProcessingExecutor::RollUpApply(_) => {},
                    ResultProcessingExecutor::PatternApply(_) => {},
                    ResultProcessingExecutor::Apply(_) => {},
                },

                // Graph Operation Executors (support input chaining)
                ExecutorEnum::GraphOperation(exec) => match exec {
                    GraphOperationExecutor::Expand(e) => e.set_input($input),
                    GraphOperationExecutor::ExpandAll(e) => e.set_input($input),
                    GraphOperationExecutor::Traverse(e) => e.set_input($input),
                    GraphOperationExecutor::ShortestPath(e) => e.set_input($input),
                    GraphOperationExecutor::MultiShortestPath(e) => e.set_input($input),
                    _ => {}
                },

                // Data Modification Executors (only PipeDelete)
                ExecutorEnum::PipeDelete(exec) => exec.set_input($input),

                // All other variants
                _ => {}
            }
        };
    }

    /// Generate get_input match arms for InputExecutor trait
    /// Only applies to variants supporting input chaining, returns Option
    macro_rules! call_input_executor_get_input {
        ($self:expr) => {
            match $self {
                // Basic Relational Operators
                ExecutorEnum::Filter(exec) => exec.get_input(),
                ExecutorEnum::Project(exec) => exec.get_input(),

                // Result Processing Executors
                ExecutorEnum::ResultProcessing(exec) => match exec {
                    ResultProcessingExecutor::Limit(e) => e.get_input(),
                    ResultProcessingExecutor::Sort(e) => e.get_input(),
                    ResultProcessingExecutor::TopN(e) => e.get_input(),
                    ResultProcessingExecutor::Sample(e) => e.get_input(),
                    ResultProcessingExecutor::Dedup(e) => e.get_input(),
                    ResultProcessingExecutor::Aggregate(e) => e.get_input(),
                    ResultProcessingExecutor::GroupBy(e) => e.get_input(),
                    ResultProcessingExecutor::Having(e) => e.get_input(),
                    ResultProcessingExecutor::Window(e) => e.get_input(),
                    ResultProcessingExecutor::Materialize(e) => e.get_input(),
                    ResultProcessingExecutor::Unwind(e) => e.get_input(),
                    ResultProcessingExecutor::Remove(e) => e.get_input(),
                    // Non-input supporting executors
                    ResultProcessingExecutor::Assign(_) => None,
                    ResultProcessingExecutor::AppendVertices(_) => None,
                    ResultProcessingExecutor::RollUpApply(_) => None,
                    ResultProcessingExecutor::PatternApply(_) => None,
                    ResultProcessingExecutor::Apply(_) => None,
                },

                // Graph Operation Executors (support input chaining)
                ExecutorEnum::GraphOperation(exec) => match exec {
                    GraphOperationExecutor::Expand(e) => e.get_input(),
                    GraphOperationExecutor::ExpandAll(e) => e.get_input(),
                    GraphOperationExecutor::Traverse(e) => e.get_input(),
                    GraphOperationExecutor::ShortestPath(e) => e.get_input(),
                    GraphOperationExecutor::MultiShortestPath(e) => e.get_input(),
                    _ => None,
                },

                // Data Modification Executors (only PipeDelete)
                ExecutorEnum::PipeDelete(exec) => exec.get_input(),

                // All other variants
                _ => None,
            }
        };
    }

    pub(crate) use delegate_to_executor;
    pub(crate) use delegate_to_executor_mut;
    pub(crate) use call_input_executor_set_input;
    pub(crate) use call_input_executor_get_input;
}

use macros::{delegate_to_executor, delegate_to_executor_mut, call_input_executor_set_input, call_input_executor_get_input};
