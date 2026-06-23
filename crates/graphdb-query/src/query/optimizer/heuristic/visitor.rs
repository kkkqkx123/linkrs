//! Planned node visitor – used for rewriting child nodes
//!
//! This module provides the ChildRewriteVisitor class, which is used to traverse the plan tree and rewrite all its child nodes.
//! Eliminate duplicate code in plan_rewriter.rs by utilizing the existing PlanNodeVisitor trait.
//!
//! # Design Advantages
//!
//! Remove duplicate code: Unify the logic for rewriting child nodes.
//! Maintain type safety: Use static distribution to avoid the overhead associated with dynamic distribution.
//! Easy to expand: When adding new node types, you only need to implement the corresponding methods.
//! – Compatible with the existing architecture: Make use of the existing PlanNodeVisitor.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::plan_rewriter::PlanRewriter;
use crate::query::optimizer::heuristic::result::RewriteResult;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNodeClonable, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor;
use crate::query::planning::plan::PlanNodeEnum;

use crate::query::planning::plan::core::nodes::access::graph_scan_node::{
    EdgeIndexScanNode, GetEdgesNode, GetNeighborsNode, GetVerticesNode, ScanEdgesNode,
    ScanVerticesNode,
};
use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    AssignNode, DataCollectNode, DedupNode, MaterializeNode, PatternApplyNode, RollUpApplyNode,
    UnionNode, UnwindNode,
};
use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::{
    IntersectNode, MinusNode,
};
use crate::query::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, FullOuterJoinNode, HashInnerJoinNode, HashLeftJoinNode, InnerJoinNode,
    LeftJoinNode, RightJoinNode, SemiJoinNode,
};
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::core::nodes::operation::sample_node::SampleNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::{
    LimitNode, SortNode, TopNNode,
};
use crate::query::planning::plan::core::nodes::traversal::traversal_node::{
    AppendVerticesNode, BiExpandNode, BiTraverseNode, ExpandAllNode, ExpandNode, TraverseNode,
};

use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::{
    ArgumentNode, BeginTransactionNode, CommitNode, LoopNode, PassThroughNode, RollbackNode,
    SelectNode,
};
use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::query::planning::plan::core::nodes::data_modification::{
    DeleteEdgesNode, DeleteIndexNode, DeleteTagsNode, DeleteVerticesNode, InsertEdgesNode,
    InsertVerticesNode, PipeDeleteEdgesNode, PipeDeleteVerticesNode, UpdateEdgesNode, UpdateNode,
    UpdateVerticesNode,
};
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, FulltextManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
    UserManageNode, VectorManageNode,
};
#[cfg(feature = "qdrant")]
use crate::query::planning::plan::core::nodes::search::vector::data_access::{
    VectorLookupNode, VectorMatchNode, VectorSearchNode,
};
use crate::query::planning::plan::core::nodes::RemoveNode;

use crate::query::planning::plan::core::nodes::access::IndexScanNode;
use crate::query::planning::plan::core::nodes::management::stats_nodes::ShowStatsNode;
use crate::query::planning::plan::core::nodes::search::fulltext::data_access::{
    FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
};
use crate::query::planning::plan::core::nodes::traversal::{
    AllPathsNode, BFSShortestNode, MultiShortestPathNode, ShortestPathNode,
};

/// Child node overrides the visitor.
///
/// Traverse the plan tree and rewrite all child nodes; this is used by the rewrite_children method of PlanRewriter.
/// Implement zero-cost abstraction by using the PlanNodeVisitor trait.
pub struct ChildRewriteVisitor<'a> {
    ctx: &'a mut RewriteContext,
    rewriter: &'a PlanRewriter,
}

impl<'a> ChildRewriteVisitor<'a> {
    pub fn new(ctx: &'a mut RewriteContext, rewriter: &'a PlanRewriter) -> Self {
        Self { ctx, rewriter }
    }
}

// ============================================
// Macros for generating visitor methods
// ============================================

/// Generate a rewrite method for single-input nodes
macro_rules! impl_single_input_rewrite {
    ($($method:ident => $type:ty, $enum_variant:ident),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> Self::Result {
                let input_node = node.input().clone_plan_node();
                let node_id = self.ctx.allocate_node_id();
                let new_input = self.rewriter.rewrite_node(self.ctx, &input_node, node_id)?;
                let mut new_node = node.clone();
                new_node.set_input(new_input);
                Ok(PlanNodeEnum::$enum_variant(new_node))
            }
        )*
    };
}

/// Generate a rewrite method for binary-input nodes
macro_rules! impl_binary_input_rewrite {
    ($($method:ident => $type:ty, $enum_variant:ident),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> Self::Result {
                let left = node.left_input().clone_plan_node();
                let right = node.right_input().clone_plan_node();
                let left_id = self.ctx.allocate_node_id();
                let right_id = self.ctx.allocate_node_id();
                let new_left = self.rewriter.rewrite_node(self.ctx, &left, left_id)?;
                let new_right = self.rewriter.rewrite_node(self.ctx, &right, right_id)?;
                let mut new_node = node.clone();
                new_node.set_left_input(new_left);
                new_node.set_right_input(new_right);
                Ok(PlanNodeEnum::$enum_variant(new_node))
            }
        )*
    };
}

/// Generate a rewrite method for multiple-input nodes (using dependencies)
macro_rules! impl_multi_input_deps_rewrite {
    ($($method:ident => $type:ty, $enum_variant:ident),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> Self::Result {
                let deps: Vec<PlanNodeEnum> = node
                    .dependencies()
                    .iter()
                    .cloned()
                    .collect();
                let mut new_deps = Vec::new();
                for dep in &deps {
                    let node_id = self.ctx.allocate_node_id();
                    let new_dep = self.rewriter.rewrite_node(self.ctx, dep, node_id)?;
                    new_deps.push(new_dep);
                }
                let mut new_node = node.clone();
                new_node.set_dependencies(new_deps);
                Ok(PlanNodeEnum::$enum_variant(new_node))
            }
        )*
    };
}

/// Generate a rewrite method for multiple-input nodes (using inputs)
macro_rules! impl_multi_input_inputs_rewrite {
    ($($method:ident => $type:ty, $enum_variant:ident),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> Self::Result {
                let deps: Vec<PlanNodeEnum> = node
                    .inputs()
                    .iter()
                    .cloned()
                    .collect();
                let mut new_deps = Vec::new();
                for dep in &deps {
                    let node_id = self.ctx.allocate_node_id();
                    let new_dep = self.rewriter.rewrite_node(self.ctx, dep, node_id)?;
                    new_deps.push(new_dep);
                }
                let mut new_node = node.clone();
                *new_node.inputs_mut() = new_deps.into_iter().collect();
                Ok(PlanNodeEnum::$enum_variant(new_node))
            }
        )*
    };
}

/// Generate a rewrite method for leaf nodes (no inputs)
macro_rules! impl_leaf_node_rewrite {
    ($($method:ident => $type:ty, $enum_variant:ident),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> Self::Result {
                Ok(PlanNodeEnum::$enum_variant(node.clone()))
            }
        )*
    };
}

impl<'a> PlanNodeVisitor for ChildRewriteVisitor<'a> {
    type Result = RewriteResult<PlanNodeEnum>;

    fn visit_default(&mut self) -> RewriteResult<PlanNodeEnum> {
        unreachable!("visit_default should not be called - all node types should have specific visit methods")
    }

    // Single-input nodes
    impl_single_input_rewrite!(
        visit_filter => FilterNode, Filter,
        visit_project => ProjectNode, Project,
        visit_aggregate => AggregateNode, Aggregate,
        visit_sort => SortNode, Sort,
        visit_limit => LimitNode, Limit,
        visit_topn => TopNNode, TopN,
        visit_sample => SampleNode, Sample,
        visit_dedup => DedupNode, Dedup,
        visit_unwind => UnwindNode, Unwind,
        visit_pattern_apply => PatternApplyNode, PatternApply,
        visit_roll_up_apply => RollUpApplyNode, RollUpApply,
        visit_data_collect => DataCollectNode, DataCollect,
        visit_assign => AssignNode, Assign,
        visit_remove => RemoveNode, Remove,
    );

    // Multi-input nodes (using inputs)
    impl_multi_input_inputs_rewrite!(
        visit_expand => ExpandNode, Expand,
        visit_expand_all => ExpandAllNode, ExpandAll,
        visit_append_vertices => AppendVerticesNode, AppendVertices,
        visit_get_vertices => GetVerticesNode, GetVertices,
        visit_get_neighbors => GetNeighborsNode, GetNeighbors,
    );

    // Binary-input nodes (joins)
    impl_binary_input_rewrite!(
        visit_hash_inner_join => HashInnerJoinNode, HashInnerJoin,
        visit_hash_left_join => HashLeftJoinNode, HashLeftJoin,
        visit_inner_join => InnerJoinNode, InnerJoin,
        visit_left_join => LeftJoinNode, LeftJoin,
        visit_cross_join => CrossJoinNode, CrossJoin,
        visit_full_outer_join => FullOuterJoinNode, FullOuterJoin,
        visit_right_join => RightJoinNode, RightJoin,
        visit_semi_join => SemiJoinNode, SemiJoin,
        visit_bi_expand => BiExpandNode, BiExpand,
        visit_bi_traverse => BiTraverseNode, BiTraverse,
        visit_multi_shortest_path => MultiShortestPathNode, MultiShortestPath,
        visit_bfs_shortest => BFSShortestNode, BFSShortest,
        visit_all_paths => AllPathsNode, AllPaths,
        visit_shortest_path => ShortestPathNode, ShortestPath,
    );

    // Leaf nodes (no inputs)
    impl_leaf_node_rewrite!(
        visit_get_edges => GetEdgesNode, GetEdges,
        visit_scan_vertices => ScanVerticesNode, ScanVertices,
        visit_scan_edges => ScanEdgesNode, ScanEdges,
        visit_edge_index_scan => EdgeIndexScanNode, EdgeIndexScan,
        visit_argument => ArgumentNode, Argument,
        visit_pass_through => PassThroughNode, PassThrough,
        visit_start => StartNode, Start,
        visit_index_scan => IndexScanNode, IndexScan,
        visit_insert_vertices => InsertVerticesNode, InsertVertices,
        visit_insert_edges => InsertEdgesNode, InsertEdges,
        visit_update => UpdateNode, Update,
        visit_update_vertices => UpdateVerticesNode, UpdateVertices,
        visit_update_edges => UpdateEdgesNode, UpdateEdges,
        visit_delete_vertices => DeleteVerticesNode, DeleteVertices,
        visit_delete_edges => DeleteEdgesNode, DeleteEdges,
        visit_delete_tags => DeleteTagsNode, DeleteTags,
        visit_delete_index => DeleteIndexNode, DeleteIndex,
        visit_pipe_delete_vertices => PipeDeleteVerticesNode, PipeDeleteVertices,
        visit_pipe_delete_edges => PipeDeleteEdgesNode, PipeDeleteEdges,
        visit_show_stats => ShowStatsNode, ShowStats,
        visit_begin_transaction => BeginTransactionNode, BeginTransaction,
        visit_commit => CommitNode, Commit,
        visit_rollback => RollbackNode, Rollback,
        visit_fulltext_search => FulltextSearchNode, FulltextSearch,
        visit_fulltext_lookup => FulltextLookupNode, FulltextLookup,
        visit_match_fulltext => MatchFulltextNode, MatchFulltext,
    );

    // Management nodes (parameterized sub-enums)
    fn visit_space_manage(&mut self, node: &SpaceManageNode) -> Self::Result {
        Ok(PlanNodeEnum::SpaceManage(node.clone()))
    }

    fn visit_tag_manage(&mut self, node: &TagManageNode) -> Self::Result {
        Ok(PlanNodeEnum::TagManage(node.clone()))
    }

    fn visit_edge_manage(&mut self, node: &EdgeManageNode) -> Self::Result {
        Ok(PlanNodeEnum::EdgeManage(node.clone()))
    }

    fn visit_index_manage(&mut self, node: &IndexManageNode) -> Self::Result {
        Ok(PlanNodeEnum::IndexManage(node.clone()))
    }

    fn visit_user_manage(&mut self, node: &UserManageNode) -> Self::Result {
        Ok(PlanNodeEnum::UserManage(node.clone()))
    }

    fn visit_fulltext_manage(&mut self, node: &FulltextManageNode) -> Self::Result {
        Ok(PlanNodeEnum::FulltextManage(node.clone()))
    }

    fn visit_vector_manage(&mut self, node: &VectorManageNode) -> Self::Result {
        Ok(PlanNodeEnum::VectorManage(node.clone()))
    }

    #[cfg(feature = "qdrant")]
    fn visit_vector_search(&mut self, node: &VectorSearchNode) -> Self::Result {
        Ok(PlanNodeEnum::VectorSearch(node.clone()))
    }

    #[cfg(feature = "qdrant")]
    fn visit_vector_lookup(&mut self, node: &VectorLookupNode) -> Self::Result {
        Ok(PlanNodeEnum::VectorLookup(node.clone()))
    }

    #[cfg(feature = "qdrant")]
    fn visit_vector_match(&mut self, node: &VectorMatchNode) -> Self::Result {
        Ok(PlanNodeEnum::VectorMatch(node.clone()))
    }

    // Multi-input nodes (using dependencies)
    impl_multi_input_deps_rewrite!(
        visit_materialize => MaterializeNode, Materialize,
        visit_union => UnionNode, Union,
        visit_minus => MinusNode, Minus,
        visit_intersect => IntersectNode, Intersect,
        visit_traverse => TraverseNode, Traverse,
    );

    // Custom implementations for nodes with special handling
    fn visit_loop(&mut self, node: &LoopNode) -> Self::Result {
        if let Some(body_node) = node.body().clone() {
            let node_id = self.ctx.allocate_node_id();
            let new_body = self.rewriter.rewrite_node(self.ctx, &body_node, node_id)?;
            let mut new_node = node.clone();
            new_node.set_body(new_body);
            Ok(PlanNodeEnum::Loop(new_node))
        } else {
            Ok(PlanNodeEnum::Loop(node.clone()))
        }
    }

    fn visit_select(&mut self, node: &SelectNode) -> Self::Result {
        let mut new_node = node.clone();

        if let Some(if_branch) = node.if_branch().clone() {
            let node_id = self.ctx.allocate_node_id();
            let new_if = self.rewriter.rewrite_node(self.ctx, &if_branch, node_id)?;
            new_node.set_if_branch(new_if);
        }

        if let Some(else_branch) = node.else_branch().clone() {
            let node_id = self.ctx.allocate_node_id();
            let new_else = self
                .rewriter
                .rewrite_node(self.ctx, &else_branch, node_id)?;
            new_node.set_else_branch(new_else);
        }

        Ok(PlanNodeEnum::Select(new_node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_child_rewrite_visitor_single_input() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = ExpressionMeta::new(Expression::Literal(Value::Bool(true)));
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let start = PlanNodeEnum::Start(StartNode::new());
        let project =
            ProjectNode::new(start.clone(), vec![]).expect("Failed to create ProjectNode");
        let filter = FilterNode::new(PlanNodeEnum::Project(project), ctx_expr)
            .expect("Failed to create FilterNode");

        let mut rewrite_ctx = RewriteContext::new();
        let rewriter = PlanRewriter::new();
        let mut visitor = ChildRewriteVisitor::new(&mut rewrite_ctx, &rewriter);

        let result = visitor.visit_filter(&filter);
        assert!(result.is_ok());
    }

    #[test]
    fn test_child_rewrite_visitor_leaf_node() {
        let start = StartNode::new();
        let mut rewrite_ctx = RewriteContext::new();
        let rewriter = PlanRewriter::new();
        let mut visitor = ChildRewriteVisitor::new(&mut rewrite_ctx, &rewriter);

        let result = visitor.visit_start(&start);
        assert!(result.is_ok());
        match result.expect("The visit should not fail") {
            PlanNodeEnum::Start(_) => {}
            _ => panic!("The Start node is expected to be activated/started."),
        }
    }
}
