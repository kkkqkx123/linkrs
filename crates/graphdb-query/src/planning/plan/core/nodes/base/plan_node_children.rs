//! Implementation of PlanNode child node traversal

use super::super::control_flow::start_node::StartNode;
use super::plan_node_enum::PlanNodeEnum;
use super::plan_node_traits::{MultipleInputNode, SingleInputNode};
use crate::optimizer::cost::child_accessor::ChildAccessor;

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
            | PlanNodeEnum::ShowConfigs(_)
            | PlanNodeEnum::ShowQueries(_)
            | PlanNodeEnum::ShowSessions(_)
            | PlanNodeEnum::ShowFunctions(_)
            | PlanNodeEnum::ShowGraphs(_)
            | PlanNodeEnum::ShowMacros(_)
            | PlanNodeEnum::ShowAttachedDatabases(_)
            | PlanNodeEnum::ShowExtensions(_)
            | PlanNodeEnum::MacroManage(_)
            | PlanNodeEnum::TypeManage(_)
            | PlanNodeEnum::LoadFrom(_)
            | PlanNodeEnum::InQueryCall(_)
            | PlanNodeEnum::CopyFrom(_)
            | PlanNodeEnum::CopyTo(_)
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

            #[cfg(feature = "vector")]
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
            PlanNodeEnum::Flatten(node) => {
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
            PlanNodeEnum::Window(node) => {
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
            PlanNodeEnum::CorrelatedApply(node) => {
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

            PlanNodeEnum::WcoIntersect(node) => node.dependencies().iter().collect(),
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
            PlanNodeEnum::RecursiveCte(node) => {
                let mut children = Vec::new();
                children.push(node.anchor());
                if let Some(step) = node.step() {
                    children.push(step);
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
            PlanNodeEnum::Savepoint(_) => vec![],
            PlanNodeEnum::ReleaseSavepoint(_) => vec![],
        }
    }

    /// Move all children out of the node, leaving it detached.
    ///
    /// The returned children are in the same order as [`children`](Self::children).
    /// Typed input slots are left holding a `Start` placeholder, so the node
    /// must be restored with [`set_children`](Self::set_children) before
    /// further use. Moving instead of cloning is what lets tree rewrites
    /// recurse without duplicating subtrees.
    pub fn take_children(&mut self) -> Vec<PlanNodeEnum> {
        let expected = self.children().len();
        let mut taken = Vec::with_capacity(expected);
        for index in 0..expected {
            if let Some(slot) = self.get_child_mut(index) {
                taken.push(std::mem::replace(
                    slot,
                    PlanNodeEnum::Start(StartNode::new()),
                ));
            }
        }
        debug_assert_eq!(taken.len(), expected);
        taken
    }

    /// Restore children previously removed with [`take_children`](Self::take_children).
    ///
    /// Mirror slots (`deps` copies of typed inputs) are re-established, so
    /// single- and double-input nodes keep one intrinsic clone per child;
    /// multi-input and control-flow nodes move without cloning.
    pub fn set_children(&mut self, children: Vec<PlanNodeEnum>) -> Result<(), String> {
        match self {
            PlanNodeEnum::Project(node) => set_single_child(node, children),
            PlanNodeEnum::Filter(node) => set_single_child(node, children),
            PlanNodeEnum::Sort(node) => set_single_child(node, children),
            PlanNodeEnum::Limit(node) => set_single_child(node, children),
            PlanNodeEnum::TopN(node) => set_single_child(node, children),
            PlanNodeEnum::Sample(node) => set_single_child(node, children),
            PlanNodeEnum::Flatten(node) => set_single_child(node, children),
            PlanNodeEnum::Dedup(node) => set_single_child(node, children),
            PlanNodeEnum::DataCollect(node) => set_single_child(node, children),
            PlanNodeEnum::Aggregate(node) => set_single_child(node, children),
            PlanNodeEnum::Window(node) => set_single_child(node, children),
            PlanNodeEnum::Unwind(node) => set_single_child(node, children),
            PlanNodeEnum::Assign(node) => set_single_child(node, children),
            PlanNodeEnum::PatternApply(node) => set_single_child(node, children),
            PlanNodeEnum::CorrelatedApply(node) => set_single_child(node, children),
            PlanNodeEnum::RollUpApply(node) => set_single_child(node, children),
            PlanNodeEnum::Remove(node) => set_single_child(node, children),
            PlanNodeEnum::Materialize(node) => set_single_child(node, children),
            PlanNodeEnum::Traverse(node) => set_single_child(node, children),
            PlanNodeEnum::PipeDeleteVertices(node) => set_single_child(node, children),
            PlanNodeEnum::PipeDeleteEdges(node) => set_single_child(node, children),
            PlanNodeEnum::InnerJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::LeftJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::RightJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::CrossJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::FullOuterJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::SemiJoin(node) => set_binary_children(node, children),
            PlanNodeEnum::Apply(node) => set_binary_children(node, children),
            PlanNodeEnum::BiExpand(node) => set_binary_children(node, children),
            PlanNodeEnum::BiTraverse(node) => set_binary_children(node, children),
            PlanNodeEnum::Expand(node) => {
                *node.inputs_mut() = children;
                Ok(())
            }
            PlanNodeEnum::ExpandAll(node) => {
                *node.inputs_mut() = children;
                Ok(())
            }
            PlanNodeEnum::AppendVertices(node) => {
                *node.inputs_mut() = children;
                Ok(())
            }
            PlanNodeEnum::Union(node) => {
                *node.dependencies_mut() = children;
                Ok(())
            }
            PlanNodeEnum::Minus(node) => {
                *node.dependencies_mut() = children;
                Ok(())
            }
            PlanNodeEnum::Intersect(node) => {
                *node.dependencies_mut() = children;
                Ok(())
            }
            PlanNodeEnum::WcoIntersect(node) => {
                *node.dependencies_mut() = children;
                Ok(())
            }
            PlanNodeEnum::Loop(node) => {
                if children.len() > 1 {
                    return Err("set_children: Loop expects at most 1 child".to_string());
                }
                if let Some(child) = children.into_iter().next() {
                    node.set_body(child);
                }
                Ok(())
            }
            PlanNodeEnum::RecursiveCte(node) => {
                // Children are [anchor, step?], mirroring `children()`.
                if children.is_empty() || children.len() > 2 {
                    return Err("set_children: RecursiveCte expects 1 or 2 children".to_string());
                }
                let mut children = children.into_iter();
                node.set_anchor(children.next().expect("checked length"));
                if let Some(step) = children.next() {
                    node.set_step(step);
                }
                Ok(())
            }
            PlanNodeEnum::Select(node) => {
                let has_if = node.if_branch().is_some();
                let has_else = node.else_branch().is_some();
                let expected = usize::from(has_if) + usize::from(has_else);
                if children.len() != expected {
                    return Err(format!(
                        "set_children: Select expects {expected} children, got {}",
                        children.len()
                    ));
                }
                let mut children = children.into_iter();
                if has_if {
                    node.set_if_branch(children.next().expect("checked length"));
                }
                if has_else {
                    node.set_else_branch(children.next().expect("checked length"));
                }
                Ok(())
            }
            other => {
                if children.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "set_children: {} takes no children, got {}",
                        other.name(),
                        children.len()
                    ))
                }
            }
        }
    }
}

/// Restore the single child of a single-input node through its setter.
fn set_single_child<N>(node: &mut N, children: Vec<PlanNodeEnum>) -> Result<(), String>
where
    N: SingleInputNode,
{
    if children.len() != 1 {
        return Err(format!(
            "set_children: {} expects 1 child, got {}",
            node.name(),
            children.len()
        ));
    }
    node.set_input(children.into_iter().next().expect("checked length"));
    Ok(())
}

/// Restore both children of a binary node through its setters.
fn set_binary_children<N>(node: &mut N, children: Vec<PlanNodeEnum>) -> Result<(), String>
where
    N: super::plan_node_traits::BinaryInputNode,
{
    if children.len() != 2 {
        return Err(format!(
            "set_children: {} requires 2 children, got {}",
            node.name(),
            children.len()
        ));
    }
    let mut children = children.into_iter();
    let left = children.next().expect("checked length");
    let right = children.next().expect("checked length");
    node.set_left_input(left);
    node.set_right_input(right);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode;

    fn start_leaf() -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn single_input_take_set_roundtrip_reuses_shell() {
        let mut node = PlanNodeEnum::Flatten(FlattenNode::new(start_leaf(), 0).expect("flatten"));
        let shell_id = node.id();
        let taken = node.take_children();
        assert_eq!(taken.len(), 1);
        assert!(matches!(taken[0], PlanNodeEnum::Start(_)));
        node.set_children(taken).expect("set");
        assert_eq!(node.id(), shell_id);
        assert_eq!(node.children().len(), 1);
        assert!(matches!(node.children()[0], PlanNodeEnum::Start(_)));
    }

    #[test]
    fn join_take_set_restores_both_sides_and_mirrors() {
        let mut node = PlanNodeEnum::InnerJoin(
            InnerJoinNode::new(start_leaf(), start_leaf(), vec![], vec![]).expect("join"),
        );
        let taken = node.take_children();
        assert_eq!(taken.len(), 2);
        node.set_children(taken).expect("set");
        assert_eq!(node.children().len(), 2);
        assert_eq!(node.dependencies_ref().len(), 2);
    }

    #[test]
    fn union_take_set_keeps_both_deps() {
        let mut node =
            PlanNodeEnum::Union(UnionNode::new(start_leaf(), start_leaf(), true).expect("union"));
        let taken = node.take_children();
        assert_eq!(taken.len(), 2);
        node.set_children(taken).expect("set");
        // Note: `PlanNodeEnum::dependencies_ref()` reports only the typed
        // `input` mirror for Union; the inherent `deps` slice holds both.
        let PlanNodeEnum::Union(union) = &node else {
            panic!("expected union");
        };
        assert_eq!(union.dependencies().len(), 2);
        assert_eq!(node.children().len(), 2);
    }

    #[test]
    fn set_children_rejects_wrong_count_without_partial_write() {
        let mut node = PlanNodeEnum::Flatten(FlattenNode::new(start_leaf(), 0).expect("flatten"));
        assert!(node.set_children(vec![]).is_err());
        assert!(node.set_children(vec![start_leaf(), start_leaf()]).is_err());
        // The detached shell is untouched by the rejected writes.
        assert_eq!(node.children().len(), 1);
    }
}
