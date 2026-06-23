//! Implementation of the PlanNode operation
//!
//! Implementing various operation methods for PlanNodeEnum

use std::borrow::Cow;

use super::plan_node_enum::PlanNodeEnum;
use super::plan_node_traits::{MultipleInputNode, PlanNode, SingleInputNode};

macro_rules! match_all_nodes_with_default {
    ($self:expr, $method:ident, $default:expr) => {
        match $self {
            PlanNodeEnum::Start(node) => node.$method(),
            PlanNodeEnum::Project(node) => node.$method(),
            PlanNodeEnum::Sort(node) => node.$method(),
            PlanNodeEnum::Limit(node) => node.$method(),
            PlanNodeEnum::TopN(node) => node.$method(),
            PlanNodeEnum::Sample(node) => node.$method(),
            PlanNodeEnum::InnerJoin(node) => node.$method(),
            PlanNodeEnum::LeftJoin(node) => node.$method(),
            PlanNodeEnum::RightJoin(node) => node.$method(),
            PlanNodeEnum::CrossJoin(node) => node.$method(),
            PlanNodeEnum::HashInnerJoin(node) => node.$method(),
            PlanNodeEnum::HashLeftJoin(node) => node.$method(),
            PlanNodeEnum::FullOuterJoin(node) => node.$method(),
            PlanNodeEnum::SemiJoin(node) => node.$method(),
            PlanNodeEnum::IndexScan(node) => node.$method(),
            PlanNodeEnum::EdgeIndexScan(node) => node.$method(),
            PlanNodeEnum::GetVertices(node) => node.$method(),
            PlanNodeEnum::GetEdges(node) => node.$method(),
            PlanNodeEnum::GetNeighbors(node) => node.$method(),
            PlanNodeEnum::ScanVertices(node) => node.$method(),
            PlanNodeEnum::ScanEdges(node) => node.$method(),
            PlanNodeEnum::Expand(node) => node.$method(),
            PlanNodeEnum::ExpandAll(node) => node.$method(),
            PlanNodeEnum::Traverse(node) => node.$method(),
            PlanNodeEnum::AppendVertices(node) => node.$method(),
            PlanNodeEnum::BiExpand(node) => node.$method(),
            PlanNodeEnum::BiTraverse(node) => node.$method(),
            PlanNodeEnum::Filter(node) => node.$method(),
            PlanNodeEnum::Aggregate(node) => node.$method(),
            PlanNodeEnum::Argument(node) => node.$method(),
            PlanNodeEnum::Loop(node) => node.$method(),
            PlanNodeEnum::PassThrough(node) => node.$method(),
            PlanNodeEnum::Select(node) => node.$method(),
            PlanNodeEnum::BeginTransaction(node) => node.$method(),
            PlanNodeEnum::Commit(node) => node.$method(),
            PlanNodeEnum::Rollback(node) => node.$method(),
            PlanNodeEnum::DataCollect(node) => node.$method(),
            PlanNodeEnum::Dedup(node) => node.$method(),
            PlanNodeEnum::PatternApply(node) => node.$method(),
            PlanNodeEnum::RollUpApply(node) => node.$method(),
            PlanNodeEnum::Union(node) => node.$method(),
            PlanNodeEnum::Minus(node) => node.$method(),
            PlanNodeEnum::Intersect(node) => node.$method(),
            PlanNodeEnum::Unwind(node) => node.$method(),
            PlanNodeEnum::Assign(node) => node.$method(),
            PlanNodeEnum::Apply(node) => node.$method(),
            PlanNodeEnum::MultiShortestPath(node) => node.$method(),
            PlanNodeEnum::BFSShortest(node) => node.$method(),
            PlanNodeEnum::AllPaths(node) => node.$method(),
            PlanNodeEnum::ShortestPath(node) => node.$method(),
            PlanNodeEnum::SpaceManage(node) => node.$method(),
            PlanNodeEnum::TagManage(node) => node.$method(),
            PlanNodeEnum::EdgeManage(node) => node.$method(),
            PlanNodeEnum::IndexManage(node) => node.$method(),
            PlanNodeEnum::UserManage(node) => node.$method(),
            PlanNodeEnum::FulltextManage(node) => node.$method(),
            PlanNodeEnum::VectorManage(node) => node.$method(),
            _ => $default,
        }
    };
}

impl PlanNodeEnum {
    pub fn id(&self) -> i64 {
        match_all_nodes_with_default!(self, id, 0)
    }

    pub fn name(&self) -> &'static str {
        match self {
            PlanNodeEnum::Start(_) => "Start",
            PlanNodeEnum::Project(_) => "Project",
            PlanNodeEnum::Sort(_) => "Sort",
            PlanNodeEnum::Limit(_) => "Limit",
            PlanNodeEnum::TopN(_) => "TopN",
            PlanNodeEnum::Sample(_) => "Sample",
            PlanNodeEnum::InnerJoin(_) => "InnerJoin",
            PlanNodeEnum::LeftJoin(_) => "LeftJoin",
            PlanNodeEnum::RightJoin(_) => "RightJoin",
            PlanNodeEnum::CrossJoin(_) => "CrossJoin",
            PlanNodeEnum::HashInnerJoin(_) => "HashInnerJoin",
            PlanNodeEnum::HashLeftJoin(_) => "HashLeftJoin",
            PlanNodeEnum::FullOuterJoin(_) => "FullOuterJoin",
            PlanNodeEnum::SemiJoin(_) => "SemiJoin",
            PlanNodeEnum::IndexScan(_) => "IndexScan",
            PlanNodeEnum::GetVertices(_) => "GetVertices",
            PlanNodeEnum::GetEdges(_) => "GetEdges",
            PlanNodeEnum::GetNeighbors(_) => "GetNeighbors",
            PlanNodeEnum::ScanVertices(_) => "ScanVertices",
            PlanNodeEnum::ScanEdges(_) => "ScanEdges",
            PlanNodeEnum::Expand(_) => "Expand",
            PlanNodeEnum::ExpandAll(_) => "ExpandAll",
            PlanNodeEnum::Traverse(_) => "Traverse",
            PlanNodeEnum::AppendVertices(_) => "AppendVertices",
            PlanNodeEnum::BiExpand(_) => "BiExpand",
            PlanNodeEnum::BiTraverse(_) => "BiTraverse",
            PlanNodeEnum::Filter(_) => "Filter",
            PlanNodeEnum::Aggregate(_) => "Aggregate",
            PlanNodeEnum::Argument(_) => "Argument",
            PlanNodeEnum::Loop(_) => "Loop",
            PlanNodeEnum::PassThrough(_) => "PassThrough",
            PlanNodeEnum::Select(_) => "Select",
            PlanNodeEnum::BeginTransaction(_) => "BeginTransaction",
            PlanNodeEnum::Commit(_) => "Commit",
            PlanNodeEnum::Rollback(_) => "Rollback",
            PlanNodeEnum::DataCollect(_) => "DataCollect",
            PlanNodeEnum::Dedup(_) => "Dedup",
            PlanNodeEnum::PatternApply(_) => "PatternApply",
            PlanNodeEnum::RollUpApply(_) => "RollUpApply",
            PlanNodeEnum::Union(_) => "Union",
            PlanNodeEnum::Unwind(_) => "Unwind",
            PlanNodeEnum::Assign(_) => "Assign",
            PlanNodeEnum::Apply(_) => "Apply",
            PlanNodeEnum::MultiShortestPath(_) => "MultiShortestPath",
            PlanNodeEnum::BFSShortest(_) => "BFSShortest",
            PlanNodeEnum::AllPaths(_) => "AllPaths",
            PlanNodeEnum::ShortestPath(_) => "ShortestPath",

            PlanNodeEnum::SpaceManage(node) => node.name(),
            PlanNodeEnum::TagManage(node) => node.name(),
            PlanNodeEnum::EdgeManage(node) => node.name(),
            PlanNodeEnum::IndexManage(node) => node.name(),
            PlanNodeEnum::UserManage(node) => node.name(),
            PlanNodeEnum::FulltextManage(node) => node.name(),
            PlanNodeEnum::VectorManage(node) => node.name(),

            PlanNodeEnum::ShowStats(_) => "ShowStats",
            PlanNodeEnum::InsertVertices(_) => "InsertVertices",
            PlanNodeEnum::InsertEdges(_) => "InsertEdges",
            PlanNodeEnum::Remove(_) => "Remove",
            PlanNodeEnum::Update(_) => "Update",
            PlanNodeEnum::UpdateVertices(_) => "UpdateVertices",
            PlanNodeEnum::UpdateEdges(_) => "UpdateEdges",
            PlanNodeEnum::DeleteVertices(_) => "DeleteVertices",
            PlanNodeEnum::DeleteEdges(_) => "DeleteEdges",
            PlanNodeEnum::DeleteTags(_) => "DeleteTags",
            PlanNodeEnum::DeleteIndex(_) => "DeleteIndex",
            PlanNodeEnum::Minus(_) => "Minus",
            PlanNodeEnum::Intersect(_) => "Intersect",
            PlanNodeEnum::EdgeIndexScan(_) => "EdgeIndexScan",
            PlanNodeEnum::PipeDeleteVertices(_) => "PipeDeleteVertices",
            PlanNodeEnum::PipeDeleteEdges(_) => "PipeDeleteEdges",
            PlanNodeEnum::Materialize(_) => "Materialize",
            PlanNodeEnum::FulltextSearch(_) => "FulltextSearch",
            PlanNodeEnum::FulltextLookup(_) => "FulltextLookup",
            PlanNodeEnum::MatchFulltext(_) => "MatchFulltext",
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(_) => "VectorSearch",
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorLookup(_) => "VectorLookup",
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorMatch(_) => "VectorMatch",
        }
    }

    pub fn output_var(&self) -> Option<&str> {
        match self {
            PlanNodeEnum::Start(node) => node.output_var(),
            PlanNodeEnum::Project(node) => node.output_var(),
            PlanNodeEnum::Sort(node) => node.output_var(),
            PlanNodeEnum::Limit(node) => node.output_var(),
            PlanNodeEnum::TopN(node) => node.output_var(),
            PlanNodeEnum::Sample(node) => node.output_var(),
            PlanNodeEnum::InnerJoin(node) => node.output_var(),
            PlanNodeEnum::LeftJoin(node) => node.output_var(),
            PlanNodeEnum::CrossJoin(node) => node.output_var(),
            PlanNodeEnum::HashInnerJoin(node) => node.output_var(),
            PlanNodeEnum::HashLeftJoin(node) => node.output_var(),
            PlanNodeEnum::IndexScan(node) => node.output_var(),
            PlanNodeEnum::GetVertices(node) => node.output_var(),
            PlanNodeEnum::GetEdges(node) => node.output_var(),
            PlanNodeEnum::GetNeighbors(node) => node.output_var(),
            PlanNodeEnum::ScanVertices(node) => node.output_var(),
            PlanNodeEnum::ScanEdges(node) => node.output_var(),
            PlanNodeEnum::Expand(node) => node.output_var(),
            PlanNodeEnum::ExpandAll(node) => node.output_var(),
            PlanNodeEnum::Traverse(node) => node.output_var(),
            PlanNodeEnum::AppendVertices(node) => node.output_var(),
            PlanNodeEnum::Filter(node) => node.output_var(),
            PlanNodeEnum::Aggregate(node) => node.output_var(),
            PlanNodeEnum::Argument(node) => node.output_var(),
            PlanNodeEnum::Loop(node) => node.output_var(),
            PlanNodeEnum::PassThrough(node) => node.output_var(),
            PlanNodeEnum::Select(node) => node.output_var(),
            PlanNodeEnum::BeginTransaction(node) => node.output_var(),
            PlanNodeEnum::Commit(node) => node.output_var(),
            PlanNodeEnum::Rollback(node) => node.output_var(),
            PlanNodeEnum::DataCollect(node) => node.output_var(),
            PlanNodeEnum::Dedup(node) => node.output_var(),
            PlanNodeEnum::PatternApply(node) => node.output_var(),
            PlanNodeEnum::RollUpApply(node) => node.output_var(),
            PlanNodeEnum::Union(node) => node.output_var(),
            PlanNodeEnum::Unwind(node) => node.output_var(),
            PlanNodeEnum::Assign(node) => node.output_var(),
            PlanNodeEnum::MultiShortestPath(node) => node.output_var(),
            PlanNodeEnum::BFSShortest(node) => node.output_var(),
            PlanNodeEnum::AllPaths(node) => node.output_var(),
            PlanNodeEnum::ShortestPath(node) => node.output_var(),

            _ => None,
        }
    }

    pub fn col_names(&self) -> &[String] {
        match_all_nodes_with_default!(self, col_names, &[])
    }

    pub fn dependencies(&self) -> Vec<PlanNodeEnum> {
        self.dependencies_ref().iter().map(|&n| n.clone()).collect()
    }

    pub fn dependencies_ref(&self) -> Cow<'_, [&PlanNodeEnum]> {
        match self {
            PlanNodeEnum::Start(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::IndexScan(_)
            | PlanNodeEnum::EdgeIndexScan(_)
            | PlanNodeEnum::MultiShortestPath(_)
            | PlanNodeEnum::BFSShortest(_)
            | PlanNodeEnum::AllPaths(_)
            | PlanNodeEnum::ShortestPath(_)
            | PlanNodeEnum::Argument(_)
            | PlanNodeEnum::PassThrough(_)
            | PlanNodeEnum::Select(_)
            | PlanNodeEnum::BeginTransaction(_)
            | PlanNodeEnum::Commit(_)
            | PlanNodeEnum::Rollback(_)
            | PlanNodeEnum::SpaceManage(_)
            | PlanNodeEnum::TagManage(_)
            | PlanNodeEnum::EdgeManage(_)
            | PlanNodeEnum::IndexManage(_)
            | PlanNodeEnum::UserManage(_)
            | PlanNodeEnum::FulltextManage(_)
            | PlanNodeEnum::VectorManage(_)
            | PlanNodeEnum::ShowStats(_)
            | PlanNodeEnum::InsertVertices(_)
            | PlanNodeEnum::InsertEdges(_)
            | PlanNodeEnum::DeleteVertices(_)
            | PlanNodeEnum::DeleteEdges(_)
            | PlanNodeEnum::DeleteTags(_)
            | PlanNodeEnum::DeleteIndex(_)
            | PlanNodeEnum::Update(_)
            | PlanNodeEnum::UpdateVertices(_)
            | PlanNodeEnum::UpdateEdges(_) => Cow::Borrowed(&[]),

            PlanNodeEnum::Project(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Sort(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Limit(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::TopN(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Sample(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Filter(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Aggregate(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::DataCollect(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Dedup(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::PatternApply(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::RollUpApply(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Union(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Unwind(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Assign(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Traverse(node) => Cow::Owned(vec![node.input()]),

            PlanNodeEnum::InnerJoin(node) => {
                Cow::Owned(vec![node.left_input(), node.right_input()])
            }
            PlanNodeEnum::LeftJoin(node) => Cow::Owned(vec![node.left_input(), node.right_input()]),
            PlanNodeEnum::CrossJoin(node) => {
                Cow::Owned(vec![node.left_input(), node.right_input()])
            }
            PlanNodeEnum::HashInnerJoin(node) => {
                Cow::Owned(vec![node.left_input(), node.right_input()])
            }
            PlanNodeEnum::HashLeftJoin(node) => {
                Cow::Owned(vec![node.left_input(), node.right_input()])
            }
            PlanNodeEnum::FullOuterJoin(node) => {
                Cow::Owned(vec![node.left_input(), node.right_input()])
            }

            PlanNodeEnum::Expand(node) => Cow::Owned(node.inputs().iter().collect::<Vec<_>>()),
            PlanNodeEnum::ExpandAll(node) => Cow::Owned(node.inputs().iter().collect::<Vec<_>>()),
            PlanNodeEnum::AppendVertices(node) => {
                Cow::Owned(node.inputs().iter().collect::<Vec<_>>())
            }

            PlanNodeEnum::Loop(_) => Cow::Borrowed(&[]),

            PlanNodeEnum::PipeDeleteVertices(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::PipeDeleteEdges(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Remove(node) => Cow::Owned(vec![node.input()]),
            PlanNodeEnum::Materialize(node) => Cow::Owned(vec![node.input()]),

            PlanNodeEnum::Minus(node) => Cow::Owned(vec![node.input(), node.minus_input()]),
            PlanNodeEnum::Intersect(node) => Cow::Owned(vec![node.input(), node.intersect_input()]),

            PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::SemiJoin(_)
            | PlanNodeEnum::BiExpand(_)
            | PlanNodeEnum::BiTraverse(_)
            | PlanNodeEnum::Apply(_) => Cow::Borrowed(&[]),

            PlanNodeEnum::FulltextSearch(_)
            | PlanNodeEnum::FulltextLookup(_)
            | PlanNodeEnum::MatchFulltext(_) => Cow::Borrowed(&[]),

            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(_)
            | PlanNodeEnum::VectorLookup(_)
            | PlanNodeEnum::VectorMatch(_) => Cow::Borrowed(&[]),
        }
    }

    pub fn first_dependency(&self) -> Option<PlanNodeEnum> {
        let deps = self.dependencies();
        if deps.is_empty() {
            None
        } else {
            Some(deps[0].clone())
        }
    }

    pub fn set_output_var(&mut self, var: String) {
        match self {
            PlanNodeEnum::Start(node) => node.set_output_var(var),
            PlanNodeEnum::Project(node) => node.set_output_var(var),
            PlanNodeEnum::Sort(node) => node.set_output_var(var),
            PlanNodeEnum::Limit(node) => node.set_output_var(var),
            PlanNodeEnum::TopN(node) => node.set_output_var(var),
            PlanNodeEnum::Sample(node) => node.set_output_var(var),
            PlanNodeEnum::InnerJoin(node) => node.set_output_var(var),
            PlanNodeEnum::LeftJoin(node) => node.set_output_var(var),
            PlanNodeEnum::CrossJoin(node) => node.set_output_var(var),
            PlanNodeEnum::HashInnerJoin(node) => node.set_output_var(var),
            PlanNodeEnum::HashLeftJoin(node) => node.set_output_var(var),
            PlanNodeEnum::IndexScan(node) => node.set_output_var(var),
            PlanNodeEnum::GetVertices(node) => node.set_output_var(var),
            PlanNodeEnum::GetEdges(node) => node.set_output_var(var),
            PlanNodeEnum::GetNeighbors(node) => node.set_output_var(var),
            PlanNodeEnum::ScanVertices(node) => node.set_output_var(var),
            PlanNodeEnum::ScanEdges(node) => node.set_output_var(var),
            PlanNodeEnum::Expand(node) => node.set_output_var(var),
            PlanNodeEnum::ExpandAll(node) => node.set_output_var(var),
            PlanNodeEnum::Traverse(node) => node.set_output_var(var),
            PlanNodeEnum::AppendVertices(node) => node.set_output_var(var),
            PlanNodeEnum::Filter(node) => node.set_output_var(var),
            PlanNodeEnum::Aggregate(node) => node.set_output_var(var),
            PlanNodeEnum::Argument(node) => node.set_output_var(var),
            PlanNodeEnum::Loop(node) => node.set_output_var(var),
            PlanNodeEnum::PassThrough(node) => node.set_output_var(var),
            PlanNodeEnum::Select(node) => node.set_output_var(var),
            PlanNodeEnum::BeginTransaction(node) => node.set_output_var(var),
            PlanNodeEnum::Commit(node) => node.set_output_var(var),
            PlanNodeEnum::Rollback(node) => node.set_output_var(var),
            PlanNodeEnum::DataCollect(node) => node.set_output_var(var),
            PlanNodeEnum::Dedup(node) => node.set_output_var(var),
            PlanNodeEnum::PatternApply(node) => node.set_output_var(var),
            PlanNodeEnum::RollUpApply(node) => node.set_output_var(var),
            PlanNodeEnum::Union(node) => node.set_output_var(var),
            PlanNodeEnum::Unwind(node) => node.set_output_var(var),
            PlanNodeEnum::Assign(node) => node.set_output_var(var),
            PlanNodeEnum::MultiShortestPath(node) => node.set_output_var(var),
            PlanNodeEnum::BFSShortest(node) => node.set_output_var(var),
            PlanNodeEnum::AllPaths(node) => node.set_output_var(var),
            PlanNodeEnum::ShortestPath(node) => node.set_output_var(var),
            _ => {}
        }
    }
}
