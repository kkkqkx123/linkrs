use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;

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
        without_flatten
    }

    fn visit_operator_replace(node: LogicalNodeEnum) -> LogicalNodeEnum {
        match node {
            LogicalNodeEnum::Flatten(mut flatten) => {
                let child = flatten
                    .input
                    .take()
                    .map(|b| *b)
                    .expect("flatten missing input");
                Self::visit_operator(child)
            }
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
            LogicalNodeEnum::GetVertices(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::GetVertices(n)
            }
            LogicalNodeEnum::GetNeighbors(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::GetNeighbors(n)
            }
            LogicalNodeEnum::Assign(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::Assign(n)
            }
            LogicalNodeEnum::Remove(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Remove(n)
            }
            LogicalNodeEnum::DataCollect(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::DataCollect(n)
            }
            LogicalNodeEnum::Materialize(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Materialize(n)
            }
            LogicalNodeEnum::RollUpApply(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::RollUpApply(n)
            }
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
            LogicalNodeEnum::PatternApply(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::PatternApply(n)
            }
            LogicalNodeEnum::CorrelatedApply(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::CorrelatedApply(n)
            }
            LogicalNodeEnum::Apply(mut n) => {
                let left = Self::visit_operator(n.left_input().clone());
                let right = Self::visit_operator(n.right_input().clone());
                n.set_left_input(left.clone());
                n.set_right_input(right.clone());
                LogicalNodeEnum::Apply(n)
            }
            LogicalNodeEnum::Traverse(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Traverse(n)
            }
            LogicalNodeEnum::Expand(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::Expand(n)
            }
            LogicalNodeEnum::ExpandAll(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::ExpandAll(n)
            }
            LogicalNodeEnum::AppendVertices(mut n) => {
                n.deps = n.deps.into_iter().map(Self::visit_operator).collect();
                LogicalNodeEnum::AppendVertices(n)
            }
            LogicalNodeEnum::BiExpand(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::BiExpand(n)
            }
            LogicalNodeEnum::BiTraverse(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::BiTraverse(n)
            }
            LogicalNodeEnum::MultiShortestPath(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::MultiShortestPath(n)
            }
            LogicalNodeEnum::BFSShortest(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::BFSShortest(n)
            }
            LogicalNodeEnum::AllPaths(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::AllPaths(n)
            }
            LogicalNodeEnum::ShortestPath(mut n) => {
                let left = Self::visit_operator(*n.left);
                let right = Self::visit_operator(*n.right);
                n.left = Box::new(left.clone());
                n.right = Box::new(right.clone());
                n.deps = vec![left, right];
                LogicalNodeEnum::ShortestPath(n)
            }
            LogicalNodeEnum::Unwind(mut n) => {
                if let Some(input) = n.input.take() {
                    let new_input = Self::visit_operator(*input);
                    n.set_input(new_input);
                }
                LogicalNodeEnum::Unwind(n)
            }
            LogicalNodeEnum::Select(mut n) => {
                if let Some(branch) = n.take_if_branch() {
                    n.set_if_branch(Self::visit_operator(*branch));
                }
                if let Some(branch) = n.take_else_branch() {
                    n.set_else_branch(Self::visit_operator(*branch));
                }
                LogicalNodeEnum::Select(n)
            }
            LogicalNodeEnum::Loop(mut n) => {
                if let Some(body) = n.take_body() {
                    let new_body = Self::visit_operator(*body);
                    n.set_body(new_body);
                }
                LogicalNodeEnum::Loop(n)
            }
            LogicalNodeEnum::PassThrough(n) => LogicalNodeEnum::PassThrough(n),
            LogicalNodeEnum::Argument(n) => LogicalNodeEnum::Argument(n),
            LogicalNodeEnum::Start(n) => LogicalNodeEnum::Start(n),
            LogicalNodeEnum::GetEdges(n) => LogicalNodeEnum::GetEdges(n),
            LogicalNodeEnum::ScanVertices(n) => LogicalNodeEnum::ScanVertices(n),
            LogicalNodeEnum::ScanEdges(n) => LogicalNodeEnum::ScanEdges(n),
            LogicalNodeEnum::BeginTransaction(n) => LogicalNodeEnum::BeginTransaction(n),
            LogicalNodeEnum::Commit(n) => LogicalNodeEnum::Commit(n),
            LogicalNodeEnum::Rollback(n) => LogicalNodeEnum::Rollback(n),
            LogicalNodeEnum::FulltextSearch(n) => LogicalNodeEnum::FulltextSearch(n),
            LogicalNodeEnum::FulltextLookup(n) => LogicalNodeEnum::FulltextLookup(n),
            LogicalNodeEnum::MatchFulltext(n) => LogicalNodeEnum::MatchFulltext(n),
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(n) => LogicalNodeEnum::VectorSearch(n),
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorLookup(n) => LogicalNodeEnum::VectorLookup(n),
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorMatch(n) => LogicalNodeEnum::VectorMatch(n),
        }
    }

    pub fn has_flatten_public(node: &LogicalNodeEnum) -> bool {
        Self::has_flatten(node)
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
            LogicalNodeEnum::Expand(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::ExpandAll(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::AppendVertices(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::BiExpand(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::BiTraverse(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::GetVertices(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::GetNeighbors(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Assign(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
                    || n.deps.iter().any(Self::has_flatten)
            }
            LogicalNodeEnum::Remove(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::DataCollect(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::Materialize(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::RollUpApply(n) => {
                n.input.as_ref().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::Union(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Minus(n) => n.deps.iter().any(Self::has_flatten),
            LogicalNodeEnum::Intersect(n) => n.deps.iter().any(Self::has_flatten),
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
            LogicalNodeEnum::PatternApply(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::Apply(n) => {
                Self::has_flatten(n.left_input()) || Self::has_flatten(n.right_input())
            }
            LogicalNodeEnum::MultiShortestPath(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::BFSShortest(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::AllPaths(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::ShortestPath(n) => {
                Self::has_flatten(&n.left) || Self::has_flatten(&n.right)
            }
            LogicalNodeEnum::Unwind(n) => n.input.as_ref().map_or(false, |c| Self::has_flatten(c)),
            LogicalNodeEnum::Select(n) => {
                n.if_branch().map_or(false, |c| Self::has_flatten(c))
                    || n.else_branch().map_or(false, |c| Self::has_flatten(c))
            }
            LogicalNodeEnum::Loop(n) => n.body().map_or(false, |c| Self::has_flatten(c)),
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

    #[test]
    fn remove_nested_under_assign() {
        let scan = scan();
        let flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, scan.clone()));
        let mut assign = LogicalNodeEnum::Assign(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode {
                id: next_node_id(),
                input: Some(Box::new(flatten)),
                deps: vec![],
                assignments: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        RemoveFactorizationRewriter::new().rewrite(&mut assign);
        assert!(!RemoveFactorizationRewriter::has_flatten(&assign));
    }

    #[test]
    fn remove_under_bi_traverse() {
        let left = scan();
        let right = scan();
        let flatten_left = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, left));
        let mut bi = LogicalNodeEnum::BiTraverse(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalBiTraverseNode {
                id: next_node_id(),
                left: Box::new(flatten_left),
                right: Box::new(right.clone()),
                deps: vec![],
                space_id: 1,
                left_src_var: "a".to_string(),
                right_src_var: "b".to_string(),
                edge_types: vec![],
                left_direction: graphdb_core::types::EdgeDirection::Out,
                right_direction: graphdb_core::types::EdgeDirection::Out,
                min_hops: 1,
                max_hops: 3,
                path_var: "p".to_string(),
                edge_alias: None,
                vertex_alias: None,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        RemoveFactorizationRewriter::new().rewrite(&mut bi);
        assert!(!RemoveFactorizationRewriter::has_flatten(&bi));
    }

    #[test]
    fn has_flatten_deep_loop() {
        let scan = scan();
        let flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, scan));
        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let expr = graphdb_core::Expression::Variable("x".to_string());
        let meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let cond = graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx);
        let loop_node = LogicalNodeEnum::Loop(
            crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode::new_with_body(cond, flatten),
        );
        assert!(RemoveFactorizationRewriter::has_flatten(&loop_node));
    }
}
