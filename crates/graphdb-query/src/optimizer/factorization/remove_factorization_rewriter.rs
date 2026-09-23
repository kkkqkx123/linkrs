use crate::planning::plan::factorization::FactorizationError;
use crate::planning::plan::factorization::FactorizedSchema;
use crate::planning::plan::factorization::FactorizedSchemaCompute;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_node_traits::{
    LogicalBinaryInputNode, LogicalMultipleInputNode, LogicalSingleInputNode,
};

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
    /// `LogicalFlatten` with its child, and recompute flat schemas on
    /// every operator so downstream passes see a fully flat view.
    pub fn rewrite(&self, plan: &mut LogicalNodeEnum) -> Result<(), FactorizationError> {
        let old = std::mem::take(plan);
        let (new_root, _schema) = Self::visit_operator(old)?;
        *plan = new_root;
        debug_assert!(
            !Self::has_flatten(plan),
            "RemoveFactorizationRewriter: residual LogicalFlatten after rewrite"
        );
        Ok(())
    }

    /// Bottom-up traversal returning a new tree without Flatten nodes and
    /// the flat schema for the rewritten subtree.
    fn visit_operator(
        node: LogicalNodeEnum,
    ) -> Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError> {
        Self::visit_operator_replace(node)
    }

    fn visit_operator_replace(
        node: LogicalNodeEnum,
    ) -> Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError> {
        match node {
            LogicalNodeEnum::Flatten(mut flatten) => {
                let child = flatten
                    .input
                    .take()
                    .map(|b| *b)
                    .expect("flatten missing input");
                Self::visit_operator(child)
            }
            LogicalNodeEnum::Project(n) => Self::visit_single_input(n, LogicalNodeEnum::Project),
            LogicalNodeEnum::Filter(n) => Self::visit_single_input(n, LogicalNodeEnum::Filter),
            LogicalNodeEnum::Sort(n) => Self::visit_single_input(n, LogicalNodeEnum::Sort),
            LogicalNodeEnum::Limit(n) => Self::visit_single_input(n, LogicalNodeEnum::Limit),
            LogicalNodeEnum::Skip(n) => Self::visit_single_input(n, LogicalNodeEnum::Skip),
            LogicalNodeEnum::TopN(n) => Self::visit_single_input(n, LogicalNodeEnum::TopN),
            LogicalNodeEnum::Sample(n) => Self::visit_single_input(n, LogicalNodeEnum::Sample),
            LogicalNodeEnum::Dedup(n) => Self::visit_single_input(n, LogicalNodeEnum::Dedup),
            LogicalNodeEnum::Aggregate(n) => {
                Self::visit_single_input(n, LogicalNodeEnum::Aggregate)
            }
            LogicalNodeEnum::Window(n) => Self::visit_single_input(n, LogicalNodeEnum::Window),
            LogicalNodeEnum::GetVertices(n) => Self::visit_deps(n, LogicalNodeEnum::GetVertices),
            LogicalNodeEnum::GetNeighbors(n) => Self::visit_deps(n, LogicalNodeEnum::GetNeighbors),
            LogicalNodeEnum::Assign(n) => Self::visit_single_input(n, LogicalNodeEnum::Assign),
            LogicalNodeEnum::Remove(n) => Self::visit_single_input(n, LogicalNodeEnum::Remove),
            LogicalNodeEnum::DataCollect(n) => {
                Self::visit_single_input(n, LogicalNodeEnum::DataCollect)
            }
            LogicalNodeEnum::Materialize(n) => {
                Self::visit_single_input(n, LogicalNodeEnum::Materialize)
            }
            LogicalNodeEnum::RollUpApply(n) => {
                Self::visit_single_input(n, LogicalNodeEnum::RollUpApply)
            }
            LogicalNodeEnum::Union(n) => Self::visit_deps(n, LogicalNodeEnum::Union),
            LogicalNodeEnum::Minus(n) => Self::visit_deps(n, LogicalNodeEnum::Minus),
            LogicalNodeEnum::Intersect(n) => Self::visit_deps(n, LogicalNodeEnum::Intersect),
            LogicalNodeEnum::WcoIntersect(n) => Self::visit_deps(n, LogicalNodeEnum::WcoIntersect),
            LogicalNodeEnum::InnerJoin(n) => Self::visit_binary(n, LogicalNodeEnum::InnerJoin),
            LogicalNodeEnum::LeftJoin(n) => Self::visit_binary(n, LogicalNodeEnum::LeftJoin),
            LogicalNodeEnum::RightJoin(n) => Self::visit_binary(n, LogicalNodeEnum::RightJoin),
            LogicalNodeEnum::CrossJoin(n) => Self::visit_binary(n, LogicalNodeEnum::CrossJoin),
            LogicalNodeEnum::FullOuterJoin(n) => {
                Self::visit_binary(n, LogicalNodeEnum::FullOuterJoin)
            }
            LogicalNodeEnum::SemiJoin(n) => Self::visit_binary(n, LogicalNodeEnum::SemiJoin),
            LogicalNodeEnum::PatternApply(n) => {
                Self::visit_binary(n, LogicalNodeEnum::PatternApply)
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                Self::visit_binary(n, LogicalNodeEnum::CorrelatedApply)
            }
            LogicalNodeEnum::Apply(mut n) => {
                let (left, left_schema) = Self::visit_operator(n.left_input().clone())?;
                let (right, right_schema) = Self::visit_operator(n.right_input().clone())?;
                n.set_left_input(left);
                n.set_right_input(right);
                let mut node = LogicalNodeEnum::Apply(n);
                let schema = node.compute_flat_schema(&[left_schema, right_schema])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Traverse(n) => Self::visit_single_input(n, LogicalNodeEnum::Traverse),
            LogicalNodeEnum::Expand(n) => Self::visit_deps(n, LogicalNodeEnum::Expand),
            LogicalNodeEnum::ExpandAll(n) => Self::visit_deps(n, LogicalNodeEnum::ExpandAll),
            LogicalNodeEnum::AppendVertices(n) => {
                Self::visit_deps(n, LogicalNodeEnum::AppendVertices)
            }
            LogicalNodeEnum::BiExpand(n) => Self::visit_binary(n, LogicalNodeEnum::BiExpand),
            LogicalNodeEnum::BiTraverse(n) => Self::visit_binary(n, LogicalNodeEnum::BiTraverse),
            LogicalNodeEnum::MultiShortestPath(n) => {
                Self::visit_binary(n, LogicalNodeEnum::MultiShortestPath)
            }
            LogicalNodeEnum::BFSShortest(n) => Self::visit_binary(n, LogicalNodeEnum::BFSShortest),
            LogicalNodeEnum::AllPaths(n) => Self::visit_binary(n, LogicalNodeEnum::AllPaths),
            LogicalNodeEnum::ShortestPath(n) => {
                Self::visit_binary(n, LogicalNodeEnum::ShortestPath)
            }
            LogicalNodeEnum::Unwind(n) => Self::visit_single_input(n, LogicalNodeEnum::Unwind),
            LogicalNodeEnum::Select(mut n) => {
                let mut child_schemas = Vec::new();
                if let Some(branch) = n.take_if_branch() {
                    let (new_branch, schema) = Self::visit_operator(*branch)?;
                    n.set_if_branch(new_branch);
                    child_schemas.push(schema);
                }
                if let Some(branch) = n.take_else_branch() {
                    let (new_branch, schema) = Self::visit_operator(*branch)?;
                    n.set_else_branch(new_branch);
                    child_schemas.push(schema);
                }
                let mut node = LogicalNodeEnum::Select(n);
                let schema = node.compute_flat_schema(&child_schemas)?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Loop(mut n) => {
                let mut child_schemas = Vec::new();
                if let Some(body) = n.take_body() {
                    let (new_body, schema) = Self::visit_operator(*body)?;
                    n.set_body(new_body);
                    child_schemas.push(schema);
                }
                let mut node = LogicalNodeEnum::Loop(n);
                let schema = node.compute_flat_schema(&child_schemas)?;
                Ok((node, schema))
            }
            LogicalNodeEnum::PassThrough(n) => {
                let mut node = LogicalNodeEnum::PassThrough(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Argument(n) => {
                let mut node = LogicalNodeEnum::Argument(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Start(n) => {
                let mut node = LogicalNodeEnum::Start(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::GetEdges(n) => {
                let mut node = LogicalNodeEnum::GetEdges(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::ScanVertices(n) => {
                let mut node = LogicalNodeEnum::ScanVertices(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::ScanEdges(n) => {
                let mut node = LogicalNodeEnum::ScanEdges(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::BeginTransaction(n) => {
                let mut node = LogicalNodeEnum::BeginTransaction(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Commit(n) => {
                let mut node = LogicalNodeEnum::Commit(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Rollback(n) => {
                let mut node = LogicalNodeEnum::Rollback(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::InsertVertices(n) => {
                let mut node = LogicalNodeEnum::InsertVertices(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::InsertEdges(n) => {
                let mut node = LogicalNodeEnum::InsertEdges(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::Update(n) => {
                let mut node = LogicalNodeEnum::Update(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::DeleteVertices(n) => {
                let mut node = LogicalNodeEnum::DeleteVertices(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::DeleteEdges(n) => {
                let mut node = LogicalNodeEnum::DeleteEdges(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::DeleteIndex(n) => {
                let mut node = LogicalNodeEnum::DeleteIndex(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::PipeDeleteVertices(n) => {
                let mut node = LogicalNodeEnum::PipeDeleteVertices(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::PipeDeleteEdges(n) => {
                let mut node = LogicalNodeEnum::PipeDeleteEdges(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::CopyFrom(n) => {
                let mut node = LogicalNodeEnum::CopyFrom(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::CopyTo(n) => {
                let mut node = LogicalNodeEnum::CopyTo(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::FulltextSearch(n) => {
                let mut node = LogicalNodeEnum::FulltextSearch(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::FulltextLookup(n) => {
                let mut node = LogicalNodeEnum::FulltextLookup(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            LogicalNodeEnum::MatchFulltext(n) => {
                let mut node = LogicalNodeEnum::MatchFulltext(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(n) => {
                let mut node = LogicalNodeEnum::VectorSearch(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorLookup(n) => {
                let mut node = LogicalNodeEnum::VectorLookup(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorMatch(n) => {
                let mut node = LogicalNodeEnum::VectorMatch(n);
                let schema = node.compute_flat_schema(&[])?;
                Ok((node, schema))
            }
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
            LogicalNodeEnum::Project(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Filter(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Sort(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Limit(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Skip(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::TopN(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Sample(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Dedup(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Aggregate(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Window(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Traverse(n) => n.input.as_deref().is_some_and(Self::has_flatten),
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
            LogicalNodeEnum::Assign(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Remove(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::DataCollect(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Materialize(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::RollUpApply(n) => n.input.as_deref().is_some_and(Self::has_flatten),
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
            LogicalNodeEnum::Unwind(n) => n.input.as_deref().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Select(n) => {
                n.if_branch().is_some_and(Self::has_flatten)
                    || n.else_branch().is_some_and(Self::has_flatten)
            }
            LogicalNodeEnum::Loop(n) => n.body().is_some_and(Self::has_flatten),
            LogicalNodeEnum::Flatten(_) => true,
            _ => false,
        }
    }

    /// Visit a node with exactly one optional `input` child.
    ///
    /// `#[inline(never)]` on purpose: [`Self::visit_operator_replace`] is a
    /// giant `match` whose arms each keep a large concrete node temporary
    /// alive across the recursive call, so the compiler reserves stack for
    /// the union of every arm in a single frame. Routing the uniform arms
    /// through thin helpers keeps the recursion frames small enough for the
    /// default 2 MiB test-thread stack.
    #[inline(never)]
    fn visit_single_input<N, F>(
        mut n: N,
        wrap: F,
    ) -> Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError>
    where
        N: LogicalSingleInputNode,
        F: FnOnce(N) -> LogicalNodeEnum,
    {
        let mut child_schemas = Vec::new();
        if let Some(input) = n.take_input() {
            let (new_input, schema) = Self::visit_operator(input)?;
            n.set_input(new_input);
            child_schemas.push(schema);
        }
        let mut node = wrap(n);
        let schema = node.compute_flat_schema(&child_schemas)?;
        Ok((node, schema))
    }

    /// Visit a node whose children live in the macro-generated `deps` vector.
    #[inline(never)]
    fn visit_deps<N, F>(
        mut n: N,
        wrap: F,
    ) -> Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError>
    where
        N: LogicalMultipleInputNode,
        F: FnOnce(N) -> LogicalNodeEnum,
    {
        let child_schemas: Vec<FactorizedSchema> = n
            .inputs_mut()
            .iter_mut()
            .map(|dep| {
                let old = std::mem::take(dep);
                let (new_dep, schema) = Self::visit_operator(old)?;
                *dep = new_dep;
                Ok(schema)
            })
            .collect::<Result<Vec<FactorizedSchema>, FactorizationError>>()?;
        let mut node = wrap(n);
        let schema = node.compute_flat_schema(&child_schemas)?;
        Ok((node, schema))
    }

    /// Visit a node with a boxed `left`/`right` child pair.
    #[inline(never)]
    fn visit_binary<N, F>(
        mut n: N,
        wrap: F,
    ) -> Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError>
    where
        N: LogicalBinaryInputNode,
        F: FnOnce(N) -> LogicalNodeEnum,
    {
        let left_old = std::mem::take(n.left_input_mut());
        let right_old = std::mem::take(n.right_input_mut());
        let (left, left_schema) = Self::visit_operator(left_old)?;
        let (right, right_schema) = Self::visit_operator(right_old)?;
        n.set_left_input(left);
        n.set_right_input(right);
        let mut node = wrap(n);
        let schema = node.compute_flat_schema(&[left_schema, right_schema])?;
        Ok((node, schema))
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
            index_hint: None,
            estimated_cardinality: None,
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
        rewriter.rewrite(&mut root).unwrap();
        assert!(!matches!(root, LogicalNodeEnum::Flatten(_)));
        assert_eq!(root.type_name(), "ScanVertices");
    }

    #[test]
    fn remove_nested_flatten() {
        let scan = scan();
        let f1 = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(0, scan));
        let f2 = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(1, f1));
        let mut root = f2;
        RemoveFactorizationRewriter::new()
            .rewrite(&mut root)
            .unwrap();
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
                assignments: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        RemoveFactorizationRewriter::new()
            .rewrite(&mut assign)
            .unwrap();
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
                right: Box::new(right),
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
        RemoveFactorizationRewriter::new().rewrite(&mut bi).unwrap();
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
