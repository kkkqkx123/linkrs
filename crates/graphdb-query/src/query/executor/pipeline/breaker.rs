//! Pipeline breaker classification
//!
//! Defines which plan nodes are pipeline breakers — operators that
//! consume all input before producing output, thus requiring a
//! pipeline boundary.

use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Kinds of pipeline breakers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineBreakerKind {
    Sort,
    Aggregate,
    Distinct,
    HashJoinBuild,
    Window,
    Materialize,
    SetOps,
    VariableLengthTraversal,
    ShortestPath,
    DmlDdlSink,
}

impl PipelineBreakerKind {
    pub fn name(&self) -> &'static str {
        match self {
            PipelineBreakerKind::Sort => "Sort",
            PipelineBreakerKind::Aggregate => "Aggregate",
            PipelineBreakerKind::Distinct => "Distinct",
            PipelineBreakerKind::HashJoinBuild => "HashJoinBuild",
            PipelineBreakerKind::Window => "Window",
            PipelineBreakerKind::Materialize => "Materialize",
            PipelineBreakerKind::SetOps => "SetOps",
            PipelineBreakerKind::VariableLengthTraversal => "VariableLengthTraversal",
            PipelineBreakerKind::ShortestPath => "ShortestPath",
            PipelineBreakerKind::DmlDdlSink => "DmlDdlSink",
        }
    }
}

/// Returns `Some(kind)` if the node is a pipeline breaker, `None` otherwise.
pub fn classify_breaker(node: &PlanNodeEnum) -> Option<PipelineBreakerKind> {
    match node {
        PlanNodeEnum::Sort(_) | PlanNodeEnum::TopN(_) => Some(PipelineBreakerKind::Sort),

        PlanNodeEnum::Aggregate(_) => Some(PipelineBreakerKind::Aggregate),

        PlanNodeEnum::Dedup(_) => Some(PipelineBreakerKind::Distinct),

        PlanNodeEnum::Window(_) => Some(PipelineBreakerKind::Window),

        PlanNodeEnum::Materialize(_) | PlanNodeEnum::DataCollect(_) => {
            Some(PipelineBreakerKind::Materialize)
        }

        PlanNodeEnum::Union(_)
        | PlanNodeEnum::Minus(_)
        | PlanNodeEnum::Intersect(_) => Some(PipelineBreakerKind::SetOps),

        PlanNodeEnum::HashInnerJoin(_) | PlanNodeEnum::HashLeftJoin(_) => {
            Some(PipelineBreakerKind::HashJoinBuild)
        }

        PlanNodeEnum::Traverse(n) if n.max_steps() > 1 => {
            Some(PipelineBreakerKind::VariableLengthTraversal)
        }
        PlanNodeEnum::BiExpand(_) | PlanNodeEnum::BiTraverse(_) => {
            Some(PipelineBreakerKind::VariableLengthTraversal)
        }

        PlanNodeEnum::ShortestPath(_)
        | PlanNodeEnum::BFSShortest(_)
        | PlanNodeEnum::AllPaths(_)
        | PlanNodeEnum::MultiShortestPath(_) => Some(PipelineBreakerKind::ShortestPath),

        PlanNodeEnum::InsertVertices(_)
        | PlanNodeEnum::InsertEdges(_)
        | PlanNodeEnum::DeleteVertices(_)
        | PlanNodeEnum::DeleteEdges(_)
        | PlanNodeEnum::DeleteTags(_)
        | PlanNodeEnum::DeleteIndex(_)
        | PlanNodeEnum::PipeDeleteVertices(_)
        | PlanNodeEnum::PipeDeleteEdges(_)
        | PlanNodeEnum::Update(_)
        | PlanNodeEnum::UpdateVertices(_)
        | PlanNodeEnum::UpdateEdges(_) => Some(PipelineBreakerKind::DmlDdlSink),

        PlanNodeEnum::SpaceManage(_)
        | PlanNodeEnum::TagManage(_)
        | PlanNodeEnum::EdgeManage(_)
        | PlanNodeEnum::IndexManage(_)
        | PlanNodeEnum::UserManage(_)
        | PlanNodeEnum::FulltextManage(_)
        | PlanNodeEnum::VectorManage(_) => Some(PipelineBreakerKind::DmlDdlSink),

        _ => None,
    }
}

    /// Returns `true` if the node is a source (leaf) — has no child inputs.
pub fn is_source(node: &PlanNodeEnum) -> bool {
    use PlanNodeEnum::*;
    matches!(
        node,
        Start(_)
            | ScanVertices(_)
            | ScanEdges(_)
            | Argument(_)
            | IndexScan(_)
            | EdgeIndexScan(_)
            | FulltextSearch(_)
            | FulltextLookup(_)
            | MatchFulltext(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::SortNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

    fn make_start() -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn test_sort_is_breaker() {
        let sort = PlanNodeEnum::Sort(SortNode::new(make_start(), vec![]).unwrap());
        assert_eq!(classify_breaker(&sort), Some(PipelineBreakerKind::Sort));
    }

    #[test]
    fn test_limit_is_not_breaker() {
        let limit = PlanNodeEnum::Limit(LimitNode::new(make_start(), 0, 10).unwrap());
        assert_eq!(classify_breaker(&limit), None);
    }

    #[test]
    fn test_start_is_source() {
        assert!(is_source(&make_start()));
    }
}
