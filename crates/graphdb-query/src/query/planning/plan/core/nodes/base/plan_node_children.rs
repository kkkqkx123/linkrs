//! Implementation of PlanNode child node traversal

use super::plan_node_enum::PlanNodeEnum;
use super::plan_node_traits::{MultipleInputNode, SingleInputNode};

impl PlanNodeEnum {
    pub fn children(&self) -> Vec<&PlanNodeEnum> {
        match self {
            PlanNodeEnum::Start(_)
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
            | PlanNodeEnum::UpdateEdges(_)
            | PlanNodeEnum::IndexScan(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::EdgeIndexScan(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::ShortestPath(_)
            | PlanNodeEnum::AllPaths(_)
            | PlanNodeEnum::BFSShortest(_)
            | PlanNodeEnum::MultiShortestPath(_)
            | PlanNodeEnum::FulltextSearch(_)
            | PlanNodeEnum::FulltextLookup(_)
            | PlanNodeEnum::MatchFulltext(_) => vec![],

            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(_)
            | PlanNodeEnum::VectorLookup(_)
            | PlanNodeEnum::VectorMatch(_) => vec![],

            PlanNodeEnum::Project(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Filter(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Sort(node) => vec![super::plan_node_traits::SingleInputNode::input(node)],
            PlanNodeEnum::Limit(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::TopN(node) => vec![super::plan_node_traits::SingleInputNode::input(node)],
            PlanNodeEnum::Sample(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Dedup(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::DataCollect(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Aggregate(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Unwind(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Assign(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::PatternApply(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::RollUpApply(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Remove(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Materialize(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::Traverse(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::PipeDeleteVertices(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }
            PlanNodeEnum::PipeDeleteEdges(node) => {
                vec![super::plan_node_traits::SingleInputNode::input(node)]
            }

            PlanNodeEnum::InnerJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::LeftJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::RightJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::CrossJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::HashInnerJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::HashLeftJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::FullOuterJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::SemiJoin(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],

            PlanNodeEnum::Apply(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],

            PlanNodeEnum::Expand(node) => node.inputs().iter().collect(),
            PlanNodeEnum::ExpandAll(node) => node.inputs().iter().collect(),
            PlanNodeEnum::AppendVertices(node) => node.inputs().iter().collect(),

            PlanNodeEnum::BiExpand(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],
            PlanNodeEnum::BiTraverse(node) => vec![
                super::plan_node_traits::BinaryInputNode::left_input(node),
                super::plan_node_traits::BinaryInputNode::right_input(node),
            ],

            PlanNodeEnum::Union(node) => node.dependencies().iter().collect(),
            PlanNodeEnum::Minus(node) => {
                vec![node.input(), node.minus_input()]
            }
            PlanNodeEnum::Intersect(node) => {
                vec![node.input(), node.intersect_input()]
            }

            PlanNodeEnum::Argument(_) => vec![],
            PlanNodeEnum::Loop(node) => {
                let mut children = Vec::new();
                if let Some(body) = node.body() {
                    children.push(body.as_ref());
                }
                children
            }
            PlanNodeEnum::PassThrough(_) => vec![],
            PlanNodeEnum::Select(node) => {
                let mut children = Vec::new();
                if let Some(if_branch) = node.if_branch() {
                    children.push(if_branch.as_ref());
                }
                if let Some(else_branch) = node.else_branch() {
                    children.push(else_branch.as_ref());
                }
                children
            }
            PlanNodeEnum::BeginTransaction(_) => vec![],
            PlanNodeEnum::Commit(_) => vec![],
            PlanNodeEnum::Rollback(_) => vec![],
        }
    }
}
