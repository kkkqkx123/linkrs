use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_node_traits::{LogicalNode, LogicalSingleInputNode};

/// RemoveFactorizationRewriter: flatten all groups and eliminate LogicalFlatten nodes.
///
/// Mirrors `lbug::optimizer::RemoveFactorizationRewriter` in
/// `ref/ladybug/src/optimizer/remove_factorization_rewriter.cpp`.
/// Run at the very beginning of the optimizer pipeline so that heuristic
/// and CBO passes operate on a fully flat view.
pub struct RemoveFactorizationRewriter;

impl RemoveFactorizationRewriter {
    pub fn new() -> Self {
        Self
    }

    /// Rewrite the plan in place: bottom-up traversal, replace every
    /// `LogicalFlatten` with its child. Caller is responsible for updating
    /// associated FactorizedSchemas to flat copies via `compute_flat_schema`
    /// if needed.
    pub fn rewrite(&self, plan: &mut LogicalNodeEnum) {
        let new_root = Self::visit_operator(plan.clone());
        *plan = new_root;
        debug_assert!(
            !Self::has_flatten(plan),
            "RemoveFactorizationRewriter: residual LogicalFlatten after rewrite"
        );
    }

    /// Bottom-up traversal returning a new tree without Flatten nodes.
    fn visit_operator(node: LogicalNodeEnum) -> LogicalNodeEnum {
        let without_flatten = Self::visit_operator_replace(node);
        // In a full implementation, call compute_flat_schema here.
        without_flatten
    }

    fn visit_operator_replace(node: LogicalNodeEnum) -> LogicalNodeEnum {
        match node {
            LogicalNodeEnum::Flatten(mut flatten) => {
                // Replace flatten with its child, recursively rewritten.
                let child = flatten
                    .input
                    .take()
                    .map(|b| *b)
                    .expect("flatten missing input");
                Self::visit_operator(child)
            }
            // Single-input nodes: recurse into child
            LogicalNodeEnum::Project(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Project(n)
            }
            LogicalNodeEnum::Filter(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Filter(n)
            }
            LogicalNodeEnum::Sort(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Sort(n)
            }
            LogicalNodeEnum::Limit(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Limit(n)
            }
            LogicalNodeEnum::TopN(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::TopN(n)
            }
            LogicalNodeEnum::Sample(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Sample(n)
            }
            LogicalNodeEnum::Dedup(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Dedup(n)
            }
            LogicalNodeEnum::Aggregate(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Aggregate(n)
            }
            LogicalNodeEnum::Window(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Window(n)
            }
            // Join nodes
            LogicalNodeEnum::InnerJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::InnerJoin(n)
            }
            LogicalNodeEnum::LeftJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::LeftJoin(n)
            }
            LogicalNodeEnum::RightJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::RightJoin(n)
            }
            LogicalNodeEnum::CrossJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::CrossJoin(n)
            }
            LogicalNodeEnum::FullOuterJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::FullOuterJoin(n)
            }
            LogicalNodeEnum::SemiJoin(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::SemiJoin(n)
            }
            // Traversal nodes with single input
            LogicalNodeEnum::Traverse(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Traverse(n)
            }
            // Control flow
            LogicalNodeEnum::Loop(n) => {
                // Loop body is private and not factorization-relevant; leave unchanged.
                LogicalNodeEnum::Loop(n)
            }
            // Multiple input nodes (Set ops, etc.)
            LogicalNodeEnum::Union(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::Union(n)
            }
            LogicalNodeEnum::Minus(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::Minus(n)
            }
            LogicalNodeEnum::Intersect(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::Intersect(n)
            }
            // Leaf or other nodes: return as-is
            other => other,
        }
    }

    fn has_flatten(node: &LogicalNodeEnum) -> bool {
        if matches!(node, LogicalNodeEnum::Flatten(_)) {
            return true;
        }
        match node {
            LogicalNodeEnum::Project(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Filter(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Sort(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Limit(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::TopN(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Sample(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Dedup(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Aggregate(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::Window(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Traverse(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::InnerJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::LeftJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::RightJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::CrossJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::SemiJoin(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::Union(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Minus(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Intersect(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Loop(_) => false,
            LogicalNodeEnum::Flatten(_) => true,
            _ => false,
        }
    }
}

impl Default for RemoveFactorizationRewriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

    fn scan() -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some("person".to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            output_var: None,
            col_names: vec!["a.name".to_string()],
            column_types: vec![],
        })
    }

    #[test]
    fn remove_single_flatten() {
        let scan = scan();
        let flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, scan));
        let mut root = flatten;
        let rewriter = RemoveFactorizationRewriter::new();
        rewriter.rewrite(&mut root);
        assert!(!matches!(root, LogicalNodeEnum::Flatten(_)));
        assert_eq!(root.type_name(), "ScanVertices");
    }

    #[test]
    fn remove_nested_flatten() {
        let scan = scan();
        let f1 = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, scan));
        let f2 = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(1, f1));
        let mut root = f2;
        RemoveFactorizationRewriter::new().rewrite(&mut root);
        assert_eq!(root.type_name(), "ScanVertices");
    }
}
