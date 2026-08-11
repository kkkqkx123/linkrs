//! Shared child traversal for cost-based plan rewriters.
//!
//! `rewrite_children` clones a node, rewrites every child with the supplied
//! closure, and returns the rebuilt node. It mirrors the accessors used by
//! `PlanNodeEnum::children` but with mutation support, so rewriters only
//! implement their per-node decision and delegate recursion here.

use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    BinaryInputNode, MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Rewrite all children of `node` with `f` and return the rebuilt node.
///
/// Node types without children (leaves, DDL, management) are returned
/// unchanged, as is any variant without a mutation accessor.
pub fn rewrite_children(
    node: &PlanNodeEnum,
    f: &mut impl FnMut(&PlanNodeEnum) -> PlanNodeEnum,
) -> PlanNodeEnum {
    use PlanNodeEnum::*;

    macro_rules! rewrite_single {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            let new_input = f(cloned.input());
            cloned.set_input(new_input);
            cloned
        }};
    }
    macro_rules! rewrite_binary {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            let new_left = f(cloned.left_input());
            let new_right = f(cloned.right_input());
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            cloned
        }};
    }
    macro_rules! rewrite_multiple {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            for child in cloned.inputs_mut() {
                *child = f(child);
            }
            cloned
        }};
    }

    match node {
        // Single-input operations
        Project(n) => Project(rewrite_single!(n)),
        Filter(n) => Filter(rewrite_single!(n)),
        Sort(n) => Sort(rewrite_single!(n)),
        Limit(n) => Limit(rewrite_single!(n)),
        TopN(n) => TopN(rewrite_single!(n)),
        Sample(n) => Sample(rewrite_single!(n)),
        Dedup(n) => Dedup(rewrite_single!(n)),
        DataCollect(n) => DataCollect(rewrite_single!(n)),
        Aggregate(n) => Aggregate(rewrite_single!(n)),
        Window(n) => Window(rewrite_single!(n)),
        Unwind(n) => Unwind(rewrite_single!(n)),
        Assign(n) => Assign(rewrite_single!(n)),
        Remove(n) => Remove(rewrite_single!(n)),
        PatternApply(n) => PatternApply(rewrite_single!(n)),
        RollUpApply(n) => RollUpApply(rewrite_single!(n)),
        Materialize(n) => Materialize(rewrite_single!(n)),
        Traverse(n) => Traverse(rewrite_single!(n)),

        // Binary operators
        InnerJoin(n) => InnerJoin(rewrite_binary!(n)),
        LeftJoin(n) => LeftJoin(rewrite_binary!(n)),
        RightJoin(n) => RightJoin(rewrite_binary!(n)),
        CrossJoin(n) => CrossJoin(rewrite_binary!(n)),
        FullOuterJoin(n) => FullOuterJoin(rewrite_binary!(n)),
        SemiJoin(n) => SemiJoin(rewrite_binary!(n)),
        Apply(n) => Apply(rewrite_binary!(n)),
        BiExpand(n) => BiExpand(rewrite_binary!(n)),
        BiTraverse(n) => BiTraverse(rewrite_binary!(n)),

        // Multiple-input operators
        Expand(n) => Expand(rewrite_multiple!(n)),
        ExpandAll(n) => ExpandAll(rewrite_multiple!(n)),
        AppendVertices(n) => AppendVertices(rewrite_multiple!(n)),

        // Dependency-based operators: deps hold the single input clone plus
        // the second input; both the `input` field and the deps are rewritten
        // so the arena converter (which reads `input()`/`union_input()`) sees
        // the rewritten subtrees.  `set_input` must NOT be used here: it
        // truncates `deps` to the single input, destroying `deps[1]`.
        Union(n) => {
            let mut cloned = n.clone();
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            let first = cloned.dependencies()[0].clone();
            *cloned.input_mut() = first;
            Union(cloned)
        }
        Minus(n) => {
            let mut cloned = n.clone();
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            let first = cloned.dependencies()[0].clone();
            *cloned.input_mut() = first;
            Minus(cloned)
        }
        Intersect(n) => {
            let mut cloned = n.clone();
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            let first = cloned.dependencies()[0].clone();
            *cloned.input_mut() = first;
            Intersect(cloned)
        }

        _ => node.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;
    use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::{
        IntersectNode, MinusNode,
    };

    /// Regression test for the `set_input` truncation bug on
    /// `Union`/`Minus`/`Intersect`: `rewrite_children` previously went through
    /// `rewrite_single!`, which called `set_input`.  For dependency-based
    /// nodes that macro clears `deps` (`set_input` calls `deps.clear()`), so
    /// `union_input()` / `minus_input()` / `intersect_input()` (which index
    /// `deps[1]`) would either panic on out-of-bounds or silently drop the
    /// second subtree from the converted plan.  The current inline path
    /// rewrites every dependency in-place and only reassigns `input_mut()`,
    /// keeping both inputs intact.
    fn assert_two_deps_rewritten<F>(make_node: F)
    where
        F: Fn(
            PlanNodeEnum,
            PlanNodeEnum,
        ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError>,
    {
        let left = PlanNodeEnum::Start(StartNode::new());
        let right = PlanNodeEnum::Start(StartNode::new());
        let node = make_node(left, right).expect("node builds");

        // Identity closure that records how many children were rewritten.
        // We do not change the children here: the regression target is whether
        // `rewrite_children` *visits* both deps and leaves them intact, not
        // the rewrite result itself.
        let mut visited = 0usize;
        let rewritten = rewrite_children(&node, &mut |child: &PlanNodeEnum| {
            visited += 1;
            child.clone()
        });

        // Two deps means two recursive calls — the second subtree must not be
        // dropped by `set_input`.
        assert_eq!(visited, 2, "both deps must be rewritten");

        let (deps_len, second_input_is_start) = match rewritten {
            PlanNodeEnum::Union(n) => (
                n.dependencies().len(),
                matches!(n.union_input(), PlanNodeEnum::Start(_)),
            ),
            PlanNodeEnum::Minus(n) => (
                n.dependencies().len(),
                matches!(n.minus_input(), PlanNodeEnum::Start(_)),
            ),
            PlanNodeEnum::Intersect(n) => (
                n.dependencies().len(),
                matches!(n.intersect_input(), PlanNodeEnum::Start(_)),
            ),
            other => panic!("expected Union/Minus/Intersect, got {:?}", other),
        };
        assert_eq!(deps_len, 2, "deps must keep both subtrees");
        // The second input remained a Start node (closure was identity).
        assert!(
            second_input_is_start,
            "union_input/minus_input/intersect_input survived the rewrite"
        );
    }

    #[test]
    fn rewrite_children_keeps_both_union_deps() {
        assert_two_deps_rewritten(|left, right| {
            UnionNode::new(left, right, true).map(PlanNodeEnum::Union)
        });
    }

    #[test]
    fn rewrite_children_keeps_both_minus_deps() {
        assert_two_deps_rewritten(|left, right| {
            MinusNode::new(left, right).map(PlanNodeEnum::Minus)
        });
    }

    #[test]
    fn rewrite_children_keeps_both_intersect_deps() {
        assert_two_deps_rewritten(|left, right| {
            IntersectNode::new(left, right).map(PlanNodeEnum::Intersect)
        });
    }
}
