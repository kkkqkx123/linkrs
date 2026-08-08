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
        HashInnerJoin(n) => HashInnerJoin(rewrite_binary!(n)),
        HashLeftJoin(n) => HashLeftJoin(rewrite_binary!(n)),
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
        // the rewritten subtrees.
        Union(n) => {
            let mut cloned = rewrite_single!(n);
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            Union(cloned)
        }
        Minus(n) => {
            let mut cloned = rewrite_single!(n);
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            Minus(cloned)
        }
        Intersect(n) => {
            let mut cloned = rewrite_single!(n);
            for child in cloned.dependencies_mut() {
                *child = f(child);
            }
            Intersect(cloned)
        }

        _ => node.clone(),
    }
}
