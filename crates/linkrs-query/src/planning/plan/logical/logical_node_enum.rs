//! LogicalNodeEnum: pure logical operator tree.
//!
//! This enum contains only logical operators — no physical execution choices
//! (no IndexScan, InnerJoin, etc.).  It is the single logical fact source
//! consumed by the optimizer and the physical converter.
//!
//! Type invariant: physical algorithms cannot appear in this enum.  The
//! physical converter is the only component that may introduce physical
//! operators (IndexScan, InnerJoin, etc.) when producing a PhysicalPlan.

use super::logical_nodes::access::{
    LogicalGetEdgesNode, LogicalGetNeighborsNode, LogicalGetVerticesNode, LogicalScanEdgesNode,
    LogicalScanVerticesNode, LogicalStartNode,
};
use super::logical_nodes::algorithm::{
    LogicalAllPathsNode, LogicalBFSShortestNode, LogicalMultiShortestPathNode,
    LogicalShortestPathNode,
};
use super::logical_nodes::control_flow::{
    LogicalArgumentNode, LogicalBeginTransactionNode, LogicalCommitNode, LogicalLoopNode,
    LogicalPassThroughNode, LogicalRollbackNode, LogicalSelectNode,
};
use super::logical_nodes::dml::{
    LogicalCopyFromNode, LogicalCopyToNode, LogicalDeleteEdgesNode, LogicalDeleteIndexNode,
    LogicalDeleteVerticesNode, LogicalInsertEdgesNode, LogicalInsertVerticesNode,
    LogicalPipeDeleteEdgesNode, LogicalPipeDeleteVerticesNode, LogicalUpdateNode,
};
use super::logical_nodes::flatten::LogicalFlattenNode;
use super::logical_nodes::graph_ops::{
    LogicalApplyNode, LogicalAssignNode, LogicalCorrelatedApplyNode, LogicalDataCollectNode,
    LogicalIntersectNode, LogicalMaterializeNode, LogicalMinusNode, LogicalPatternApplyNode,
    LogicalRollUpApplyNode, LogicalUnionNode, LogicalUnwindNode,
};
use super::logical_nodes::join::{
    LogicalCrossJoinNode, LogicalFullOuterJoinNode, LogicalInnerJoinNode, LogicalLeftJoinNode,
    LogicalRightJoinNode, LogicalSemiJoinNode,
};
use super::logical_nodes::operation::{
    LogicalAggregateNode, LogicalDedupNode, LogicalFilterNode, LogicalLimitNode,
    LogicalProjectNode, LogicalSampleNode, LogicalSkipNode, LogicalSortNode, LogicalTopNNode,
    LogicalWindowNode,
};
use super::logical_nodes::search::{
    LogicalFulltextLookupNode, LogicalFulltextSearchNode, LogicalMatchFulltextNode,
};
#[cfg(feature = "vector")]
use super::logical_nodes::search::{
    LogicalVectorLookupNode, LogicalVectorMatchNode, LogicalVectorSearchNode,
};
use super::logical_nodes::traversal::{
    LogicalAppendVerticesNode, LogicalBiExpandNode, LogicalBiTraverseNode, LogicalExpandAllNode,
    LogicalExpandNode, LogicalTraverseNode,
};
use super::logical_nodes::wco_intersect::LogicalWcoIntersectNode;

#[derive(Debug, Clone)]
pub enum LogicalNodeEnum {
    // Access nodes
    Start(LogicalStartNode),
    GetVertices(LogicalGetVerticesNode),
    GetEdges(LogicalGetEdgesNode),
    GetNeighbors(LogicalGetNeighborsNode),
    ScanVertices(LogicalScanVerticesNode),
    ScanEdges(LogicalScanEdgesNode),

    // Operation nodes
    Project(LogicalProjectNode),
    Filter(LogicalFilterNode),
    Sort(LogicalSortNode),
    Limit(LogicalLimitNode),
    Skip(LogicalSkipNode),
    TopN(LogicalTopNNode),
    Sample(LogicalSampleNode),
    Dedup(LogicalDedupNode),
    Aggregate(LogicalAggregateNode),
    Window(LogicalWindowNode),

    // Join nodes
    InnerJoin(LogicalInnerJoinNode),
    LeftJoin(LogicalLeftJoinNode),
    RightJoin(LogicalRightJoinNode),
    CrossJoin(LogicalCrossJoinNode),
    FullOuterJoin(LogicalFullOuterJoinNode),
    SemiJoin(LogicalSemiJoinNode),

    // Traversal nodes
    Expand(LogicalExpandNode),
    ExpandAll(LogicalExpandAllNode),
    Traverse(LogicalTraverseNode),
    AppendVertices(LogicalAppendVerticesNode),
    BiExpand(LogicalBiExpandNode),
    BiTraverse(LogicalBiTraverseNode),

    // Control flow nodes
    Argument(LogicalArgumentNode),
    Loop(LogicalLoopNode),
    PassThrough(LogicalPassThroughNode),
    Select(LogicalSelectNode),
    BeginTransaction(LogicalBeginTransactionNode),
    Commit(LogicalCommitNode),
    Rollback(LogicalRollbackNode),

    // Data processing nodes
    DataCollect(LogicalDataCollectNode),
    PatternApply(LogicalPatternApplyNode),
    RollUpApply(LogicalRollUpApplyNode),
    CorrelatedApply(LogicalCorrelatedApplyNode),
    Union(LogicalUnionNode),
    Minus(LogicalMinusNode),
    Intersect(LogicalIntersectNode),
    Unwind(LogicalUnwindNode),
    Materialize(LogicalMaterializeNode),
    Assign(LogicalAssignNode),
    Apply(LogicalApplyNode),

    // Data modification nodes
    InsertVertices(LogicalInsertVerticesNode),
    InsertEdges(LogicalInsertEdgesNode),
    Update(LogicalUpdateNode),
    DeleteVertices(LogicalDeleteVerticesNode),
    DeleteEdges(LogicalDeleteEdgesNode),
    DeleteIndex(LogicalDeleteIndexNode),
    PipeDeleteVertices(LogicalPipeDeleteVerticesNode),
    PipeDeleteEdges(LogicalPipeDeleteEdgesNode),
    CopyFrom(LogicalCopyFromNode),
    CopyTo(LogicalCopyToNode),

    // Algorithm nodes
    MultiShortestPath(LogicalMultiShortestPathNode),
    BFSShortest(LogicalBFSShortestNode),
    AllPaths(LogicalAllPathsNode),
    ShortestPath(LogicalShortestPathNode),

    // Search nodes
    FulltextSearch(LogicalFulltextSearchNode),
    FulltextLookup(LogicalFulltextLookupNode),
    MatchFulltext(LogicalMatchFulltextNode),

    // Vector search nodes
    #[cfg(feature = "vector")]
    VectorSearch(LogicalVectorSearchNode),
    #[cfg(feature = "vector")]
    VectorLookup(LogicalVectorLookupNode),
    #[cfg(feature = "vector")]
    VectorMatch(LogicalVectorMatchNode),

    // Factorization
    Flatten(LogicalFlattenNode),

    // Worst-case optimal multi-way join
    WcoIntersect(LogicalWcoIntersectNode),
}

/// Sentinel default: `PassThrough` with id 0. Used only by
/// `std::mem::take` / `std::mem::replace` in the factorization rewriter
/// to move a child subtree out of a `Box` without cloning.
impl Default for LogicalNodeEnum {
    fn default() -> Self {
        Self::PassThrough(super::logical_nodes::control_flow::LogicalPassThroughNode {
            id: 0,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }
}

impl LogicalNodeEnum {
    pub fn id(&self) -> i64 {
        match self {
            Self::Start(n) => n.id(),
            Self::GetVertices(n) => n.id(),
            Self::GetEdges(n) => n.id(),
            Self::GetNeighbors(n) => n.id(),
            Self::ScanVertices(n) => n.id(),
            Self::ScanEdges(n) => n.id(),
            Self::Project(n) => n.id(),
            Self::Filter(n) => n.id(),
            Self::Sort(n) => n.id(),
            Self::Limit(n) => n.id(),
            Self::Skip(n) => n.id(),
            Self::TopN(n) => n.id(),
            Self::Sample(n) => n.id(),
            Self::Dedup(n) => n.id(),
            Self::Aggregate(n) => n.id(),
            Self::Window(n) => n.id(),
            Self::InnerJoin(n) => n.id(),
            Self::LeftJoin(n) => n.id(),
            Self::RightJoin(n) => n.id(),
            Self::CrossJoin(n) => n.id(),
            Self::FullOuterJoin(n) => n.id(),
            Self::SemiJoin(n) => n.id(),
            Self::Expand(n) => n.id(),
            Self::ExpandAll(n) => n.id(),
            Self::Traverse(n) => n.id(),
            Self::AppendVertices(n) => n.id(),
            Self::BiExpand(n) => n.id(),
            Self::BiTraverse(n) => n.id(),
            Self::Argument(n) => n.id(),
            Self::Loop(n) => n.id(),
            Self::PassThrough(n) => n.id(),
            Self::Select(n) => n.id(),
            Self::BeginTransaction(n) => n.id(),
            Self::Commit(n) => n.id(),
            Self::Rollback(n) => n.id(),
            Self::DataCollect(n) => n.id(),
            Self::PatternApply(n) => n.id(),
            Self::RollUpApply(n) => n.id(),
            Self::CorrelatedApply(n) => n.id(),
            Self::Union(n) => n.id(),
            Self::Minus(n) => n.id(),
            Self::Intersect(n) => n.id(),
            Self::Unwind(n) => n.id(),
            Self::Materialize(n) => n.id(),
            Self::Assign(n) => n.id(),
            Self::Apply(n) => n.id(),
            Self::InsertVertices(n) => n.id(),
            Self::InsertEdges(n) => n.id(),
            Self::Update(n) => n.id(),
            Self::DeleteVertices(n) => n.id(),
            Self::DeleteEdges(n) => n.id(),
            Self::DeleteIndex(n) => n.id(),
            Self::PipeDeleteVertices(n) => n.id(),
            Self::PipeDeleteEdges(n) => n.id(),
            Self::CopyFrom(n) => n.id(),
            Self::CopyTo(n) => n.id(),
            Self::MultiShortestPath(n) => n.id(),
            Self::BFSShortest(n) => n.id(),
            Self::AllPaths(n) => n.id(),
            Self::ShortestPath(n) => n.id(),
            Self::FulltextSearch(n) => n.id(),
            Self::FulltextLookup(n) => n.id(),
            Self::MatchFulltext(n) => n.id(),
            #[cfg(feature = "vector")]
            Self::VectorSearch(n) => n.id(),
            #[cfg(feature = "vector")]
            Self::VectorLookup(n) => n.id(),
            #[cfg(feature = "vector")]
            Self::VectorMatch(n) => n.id(),
            Self::Flatten(n) => n.id(),
            Self::WcoIntersect(n) => n.id(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Start(_) => "Start",
            Self::GetVertices(_) => "GetVertices",
            Self::GetEdges(_) => "GetEdges",
            Self::GetNeighbors(_) => "GetNeighbors",
            Self::ScanVertices(_) => "ScanVertices",
            Self::ScanEdges(_) => "ScanEdges",
            Self::Project(_) => "Project",
            Self::Filter(_) => "Filter",
            Self::Sort(_) => "Sort",
            Self::Limit(_) => "Limit",
            Self::Skip(_) => "Skip",
            Self::TopN(_) => "TopN",
            Self::Sample(_) => "Sample",
            Self::Dedup(_) => "Dedup",
            Self::Aggregate(_) => "Aggregate",
            Self::Window(_) => "Window",
            Self::InnerJoin(_) => "InnerJoin",
            Self::LeftJoin(_) => "LeftJoin",
            Self::RightJoin(_) => "RightJoin",
            Self::CrossJoin(_) => "CrossJoin",
            Self::FullOuterJoin(_) => "FullOuterJoin",
            Self::SemiJoin(_) => "SemiJoin",
            Self::Expand(_) => "Expand",
            Self::ExpandAll(_) => "ExpandAll",
            Self::Traverse(_) => "Traverse",
            Self::AppendVertices(_) => "AppendVertices",
            Self::BiExpand(_) => "BiExpand",
            Self::BiTraverse(_) => "BiTraverse",
            Self::Argument(_) => "Argument",
            Self::Loop(_) => "Loop",
            Self::PassThrough(_) => "PassThrough",
            Self::Select(_) => "Select",
            Self::BeginTransaction(_) => "BeginTransaction",
            Self::Commit(_) => "Commit",
            Self::Rollback(_) => "Rollback",
            Self::DataCollect(_) => "DataCollect",
            Self::PatternApply(_) => "PatternApply",
            Self::RollUpApply(_) => "RollUpApply",
            Self::CorrelatedApply(_) => "CorrelatedApply",
            Self::Union(_) => "Union",
            Self::Minus(_) => "Minus",
            Self::Intersect(_) => "Intersect",
            Self::Unwind(_) => "Unwind",
            Self::Materialize(_) => "Materialize",
            Self::Assign(_) => "Assign",
            Self::Apply(_) => "Apply",
            Self::InsertVertices(_) => "InsertVertices",
            Self::InsertEdges(_) => "InsertEdges",
            Self::Update(_) => "Update",
            Self::DeleteVertices(_) => "DeleteVertices",
            Self::DeleteEdges(_) => "DeleteEdges",
            Self::DeleteIndex(_) => "DeleteIndex",
            Self::PipeDeleteVertices(_) => "PipeDeleteVertices",
            Self::PipeDeleteEdges(_) => "PipeDeleteEdges",
            Self::CopyFrom(_) => "CopyFrom",
            Self::CopyTo(_) => "CopyTo",
            Self::MultiShortestPath(_) => "MultiShortestPath",
            Self::BFSShortest(_) => "BFSShortest",
            Self::AllPaths(_) => "AllPaths",
            Self::ShortestPath(_) => "ShortestPath",
            Self::FulltextSearch(_) => "FulltextSearch",
            Self::FulltextLookup(_) => "FulltextLookup",
            Self::MatchFulltext(_) => "MatchFulltext",
            #[cfg(feature = "vector")]
            Self::VectorSearch(_) => "VectorSearch",
            #[cfg(feature = "vector")]
            Self::VectorLookup(_) => "VectorLookup",
            #[cfg(feature = "vector")]
            Self::VectorMatch(_) => "VectorMatch",
            Self::Flatten(_) => "Flatten",
            Self::WcoIntersect(_) => "WcoIntersect",
        }
    }

    pub fn col_names(&self) -> &[String] {
        match self {
            Self::Start(n) => n.col_names(),
            Self::GetVertices(n) => n.col_names(),
            Self::GetEdges(n) => n.col_names(),
            Self::GetNeighbors(n) => n.col_names(),
            Self::ScanVertices(n) => n.col_names(),
            Self::ScanEdges(n) => n.col_names(),
            Self::Project(n) => n.col_names(),
            Self::Filter(n) => n.col_names(),
            Self::Sort(n) => n.col_names(),
            Self::Limit(n) => n.col_names(),
            Self::Skip(n) => n.col_names(),
            Self::TopN(n) => n.col_names(),
            Self::Sample(n) => n.col_names(),
            Self::Dedup(n) => n.col_names(),
            Self::Aggregate(n) => n.col_names(),
            Self::Window(n) => n.col_names(),
            Self::InnerJoin(n) => n.col_names(),
            Self::LeftJoin(n) => n.col_names(),
            Self::RightJoin(n) => n.col_names(),
            Self::CrossJoin(n) => n.col_names(),
            Self::FullOuterJoin(n) => n.col_names(),
            Self::SemiJoin(n) => n.col_names(),
            Self::Expand(n) => n.col_names(),
            Self::ExpandAll(n) => n.col_names(),
            Self::Traverse(n) => n.col_names(),
            Self::AppendVertices(n) => n.col_names(),
            Self::BiExpand(n) => n.col_names(),
            Self::BiTraverse(n) => n.col_names(),
            Self::Argument(n) => n.col_names(),
            Self::Loop(n) => n.col_names(),
            Self::PassThrough(n) => n.col_names(),
            Self::Select(n) => n.col_names(),
            Self::BeginTransaction(n) => n.col_names(),
            Self::Commit(n) => n.col_names(),
            Self::Rollback(n) => n.col_names(),
            Self::DataCollect(n) => n.col_names(),
            Self::PatternApply(n) => n.col_names(),
            Self::RollUpApply(n) => n.col_names(),
            Self::CorrelatedApply(n) => n.col_names(),
            Self::Union(n) => n.col_names(),
            Self::Minus(n) => n.col_names(),
            Self::Intersect(n) => n.col_names(),
            Self::Unwind(n) => n.col_names(),
            Self::Materialize(n) => n.col_names(),
            Self::Assign(n) => n.col_names(),
            Self::Apply(n) => n.col_names(),
            Self::InsertVertices(n) => n.col_names(),
            Self::InsertEdges(n) => n.col_names(),
            Self::Update(n) => n.col_names(),
            Self::DeleteVertices(n) => n.col_names(),
            Self::DeleteEdges(n) => n.col_names(),
            Self::DeleteIndex(n) => n.col_names(),
            Self::PipeDeleteVertices(n) => n.col_names(),
            Self::PipeDeleteEdges(n) => n.col_names(),
            Self::CopyFrom(n) => n.col_names(),
            Self::CopyTo(n) => n.col_names(),
            Self::MultiShortestPath(n) => n.col_names(),
            Self::BFSShortest(n) => n.col_names(),
            Self::AllPaths(n) => n.col_names(),
            Self::ShortestPath(n) => n.col_names(),
            Self::FulltextSearch(n) => n.col_names(),
            Self::FulltextLookup(n) => n.col_names(),
            Self::MatchFulltext(n) => n.col_names(),
            #[cfg(feature = "vector")]
            Self::VectorSearch(n) => n.col_names(),
            #[cfg(feature = "vector")]
            Self::VectorLookup(n) => n.col_names(),
            #[cfg(feature = "vector")]
            Self::VectorMatch(n) => n.col_names(),
            Self::Flatten(n) => n.col_names(),
            Self::WcoIntersect(n) => n.col_names(),
        }
    }

    pub fn children_ref(&self) -> Vec<&LogicalNodeEnum> {
        match self {
            Self::Flatten(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Project(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Filter(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Sort(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Limit(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Skip(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::TopN(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Sample(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Dedup(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Aggregate(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Window(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Traverse(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Assign(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::PipeDeleteVertices(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::PipeDeleteEdges(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::DataCollect(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Materialize(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::RollUpApply(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Unwind(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            Self::Select(n) => {
                let mut out = Vec::new();
                if let Some(b) = n.if_branch() {
                    out.push(b);
                }
                if let Some(b) = n.else_branch() {
                    out.push(b);
                }
                out
            }
            Self::Loop(n) => n.body().map(|b| vec![b]).unwrap_or_default(),
            Self::InnerJoin(n) => vec![&n.left, &n.right],
            Self::LeftJoin(n) => vec![&n.left, &n.right],
            Self::RightJoin(n) => vec![&n.left, &n.right],
            Self::CrossJoin(n) => vec![&n.left, &n.right],
            Self::FullOuterJoin(n) => vec![&n.left, &n.right],
            Self::SemiJoin(n) => vec![&n.left, &n.right],
            Self::PatternApply(n) => vec![&n.left, &n.right],
            Self::CorrelatedApply(n) => vec![&n.left, &n.right],
            Self::Apply(n) => vec![n.left_input(), n.right_input()],
            Self::BiExpand(n) => vec![&n.left, &n.right],
            Self::BiTraverse(n) => vec![&n.left, &n.right],
            Self::MultiShortestPath(n) => vec![&n.left, &n.right],
            Self::BFSShortest(n) => vec![&n.left, &n.right],
            Self::AllPaths(n) => vec![&n.left, &n.right],
            Self::ShortestPath(n) => vec![&n.left, &n.right],
            Self::Expand(n) => n.deps.iter().collect(),
            Self::ExpandAll(n) => n.deps.iter().collect(),
            Self::AppendVertices(n) => n.deps.iter().collect(),
            Self::GetVertices(n) => n.deps.iter().collect(),
            Self::GetNeighbors(n) => n.deps.iter().collect(),
            Self::Union(n) => n.deps.iter().collect(),
            Self::Minus(n) => n.deps.iter().collect(),
            Self::Intersect(n) => n.deps.iter().collect(),
            Self::WcoIntersect(n) => n.deps.iter().collect(),
            Self::Start(_)
            | Self::ScanVertices(_)
            | Self::ScanEdges(_)
            | Self::GetEdges(_)
            | Self::Argument(_)
            | Self::PassThrough(_)
            | Self::BeginTransaction(_)
            | Self::Commit(_)
            | Self::Rollback(_)
            | Self::InsertVertices(_)
            | Self::InsertEdges(_)
            | Self::Update(_)
            | Self::DeleteVertices(_)
            | Self::DeleteEdges(_)
            | Self::DeleteIndex(_)
            | Self::CopyFrom(_)
            | Self::CopyTo(_)
            | Self::FulltextSearch(_)
            | Self::FulltextLookup(_)
            | Self::MatchFulltext(_) => vec![],
            #[cfg(feature = "vector")]
            Self::VectorSearch(_) | Self::VectorLookup(_) | Self::VectorMatch(_) => vec![],
        }
    }

    pub fn children_mut(&mut self) -> Vec<&mut LogicalNodeEnum> {
        match self {
            Self::Flatten(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Project(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Filter(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Sort(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Limit(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Skip(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::TopN(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Sample(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Dedup(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Aggregate(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Window(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Traverse(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Unwind(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::PipeDeleteVertices(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            Self::PipeDeleteEdges(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::DataCollect(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Materialize(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::RollUpApply(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Assign(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            Self::Select(n) => {
                let mut out = Vec::new();
                if let Some(b) = n.if_branch.as_deref_mut() {
                    out.push(b);
                }
                if let Some(b) = n.else_branch.as_deref_mut() {
                    out.push(b);
                }
                out
            }
            Self::Loop(n) => n.body.as_deref_mut().map(|b| vec![b]).unwrap_or_default(),
            Self::InnerJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::LeftJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::RightJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::CrossJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::FullOuterJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::SemiJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::PatternApply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::CorrelatedApply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::Apply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::BiExpand(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::BiTraverse(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::MultiShortestPath(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::BFSShortest(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::AllPaths(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::ShortestPath(n) => vec![n.left.as_mut(), n.right.as_mut()],
            Self::Expand(n) => n.deps.iter_mut().collect(),
            Self::ExpandAll(n) => n.deps.iter_mut().collect(),
            Self::AppendVertices(n) => n.deps.iter_mut().collect(),
            Self::GetVertices(n) => n.deps.iter_mut().collect(),
            Self::GetNeighbors(n) => n.deps.iter_mut().collect(),
            Self::Union(n) => n.deps.iter_mut().collect(),
            Self::Minus(n) => n.deps.iter_mut().collect(),
            Self::Intersect(n) => n.deps.iter_mut().collect(),
            Self::WcoIntersect(n) => n.deps.iter_mut().collect(),
            Self::Start(_)
            | Self::ScanVertices(_)
            | Self::ScanEdges(_)
            | Self::GetEdges(_)
            | Self::Argument(_)
            | Self::PassThrough(_)
            | Self::BeginTransaction(_)
            | Self::Commit(_)
            | Self::Rollback(_)
            | Self::InsertVertices(_)
            | Self::InsertEdges(_)
            | Self::Update(_)
            | Self::DeleteVertices(_)
            | Self::DeleteEdges(_)
            | Self::DeleteIndex(_)
            | Self::CopyFrom(_)
            | Self::CopyTo(_)
            | Self::FulltextSearch(_)
            | Self::FulltextLookup(_)
            | Self::MatchFulltext(_) => vec![],
            #[cfg(feature = "vector")]
            Self::VectorSearch(_) | Self::VectorLookup(_) | Self::VectorMatch(_) => vec![],
        }
    }
}
