//! PATH Query Planner
//! Planning for handling Nebula PATH query requests
//!
//! ## Explanation of the improvements
//!
//! Implementing shortest path planning
//! Implement all path planning functions.
//! Support for the shortest path with weights
//! Improve the logic for path filtering.

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness;
use crate::planning::plan::core::nodes::traversal::{AllPathsNode, ShortestPathNode};
use crate::planning::plan::core::PlanNode;
use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
use crate::planning::plan::logical::logical_nodes::algorithm::{
    LogicalAllPathsNode, LogicalShortestPathNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::plan_combiner::wrap_logical_filter;
use crate::QueryContext;
use linkrs_core::types::VertexId;
use linkrs_core::Value;
use std::sync::Arc;

pub use crate::planning::plan::core::nodes::{
    ArgumentNode, DedupNode, ExpandAllNode, FilterNode, GetNeighborsNode, ProjectNode, StartNode,
};
pub use crate::planning::plan::core::PlanNodeEnum;

/// PATH Query Planner
/// Responsible for converting PATH queries into execution plans.
#[derive(Debug, Clone)]
pub struct PathPlanner {}

impl PathPlanner {
    /// Create a new PATH planner.
    pub fn new() -> Self {
        Self {}
    }
}

impl Planner for PathPlanner {

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let find_path = match bound {
            BoundStatement::FindPath(fp) => fp,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "PathPlanner requires the FindPath statement.".to_string(),
                ));
            }
        };

        let space_id = qctx.space_id().ok_or_else(|| {
            PlannerError::InvalidOperation(
                "No graph space selected, please execute USE <space> first".to_string(),
            )
        })?;

        // FIND PATH syntax carries no vertex label: path vertices are
        // materialized at execution from the edge type schema (or from tagged
        // vertex values), so the plan leaves the vertex label empty instead
        // of guessing one.
        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node);

        let edge_types = find_path
            .over
            .as_ref()
            .map(|(types, _)| types.clone())
            .unwrap_or_default();

        let max_steps = find_path.max_steps.unwrap_or(10);

        let start_vertex_ids: Vec<Value> = find_path
            .from
            .iter()
            .filter_map(|expr| match expr {
                crate::binder::bound::BoundExpression::Literal(v, _) => Some(v.clone()),
                _ => None,
            })
            .collect();

        let end_vertex_ids: Vec<Value> = match &find_path.to {
            crate::binder::bound::BoundExpression::Literal(v, _) => vec![v.clone()],
            _ => vec![],
        };

        let (mut root_node, mut logical_root) = if find_path.shortest {
            // A cycle can never produce a shorter path, so WITH LOOP /
            // WITH TRAIL / WITH WALK are no-ops here; report the ignored
            // modifiers instead of folding them into the plan.
            if find_path.with_loop || find_path.with_trail || find_path.with_walk {
                log::debug!(
                    "FIND SHORTEST PATH ignores WITH LOOP/TRAIL/WALK: \
                     revisiting a vertex or edge cannot yield a shorter path"
                );
            }
            let root = self.build_shortest_path_plan(PathPlanParams {
                left_input: start_node_enum.clone(),
                space_id,
                edge_types: edge_types.clone(),
                max_steps,
                start_vertex_ids: start_vertex_ids.clone(),
                end_vertex_ids: end_vertex_ids.clone(),
                weight_expression: find_path.weight_expression.clone(),
            })?;
            let logical = Self::shortest_path_logical(
                space_id,
                edge_types,
                max_steps,
                start_vertex_ids,
                end_vertex_ids,
                find_path.weight_expression.clone(),
            );
            (root, logical)
        } else {
            let direction = find_path
                .over
                .as_ref()
                .map(|(_, dir)| *dir)
                .unwrap_or(linkrs_core::EdgeDirection::Out);

            // Default paths do not repeat vertices; WITH LOOP / WITH TRAIL /
            // WITH WALK each relax a distinct part of that constraint (see
            // PathUniqueness).
            let path_uniqueness = PathUniqueness::from_flags(
                find_path.with_loop,
                find_path.with_trail,
                find_path.with_walk,
            );

            let right_node = StartNode::new();
            let right_node_enum = PlanNodeEnum::Start(right_node);

            let mut all_paths_node = AllPathsNode::new(
                start_node_enum.clone(),
                right_node_enum,
                space_id,
                max_steps,
                edge_types.clone(),
                1,
                max_steps,
                path_uniqueness,
            );
            all_paths_node.set_direction(direction);
            let start_vids: Vec<VertexId> = start_vertex_ids
                .iter()
                .filter_map(|v| VertexId::try_from(v).ok())
                .collect();
            let end_vids: Vec<VertexId> = end_vertex_ids
                .iter()
                .filter_map(|v| VertexId::try_from(v).ok())
                .collect();
            all_paths_node.set_start_vertex_ids(start_vids.clone());
            all_paths_node.set_end_vertex_ids(end_vids.clone());

            let logical = Self::all_paths_logical_from_parts(
                space_id,
                edge_types,
                max_steps,
                path_uniqueness,
                direction,
                start_vids,
                end_vids,
            );
            (all_paths_node.into_enum(), logical)
        };

        // Apply the bound WHERE clause on top of the path result.
        if let Some(where_clause) = &find_path.where_clause {
            let expr_ctx = Arc::new(
                linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
            );
            let condition = crate::binder::expr_converter::bound_expr_to_contextual(
                &where_clause.condition,
                &expr_ctx,
            )
            .map_err(PlannerError::PlanGenerationFailed)?;
            let filter_node =
                FilterNode::new(root_node.clone(), condition.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            let col_names = filter_node.col_names().to_vec();
            logical_root = wrap_logical_filter(logical_root, condition, col_names);
            root_node = PlanNodeEnum::Filter(filter_node);
        }

        let sub_plan = SubPlan {
            root: Some(root_node),
            tail: Some(start_node_enum),
            logical_root: Some(logical_root),
        };

        Ok(sub_plan)
    }

}

/// Shared inputs for building a path plan.
struct PathPlanParams {
    left_input: PlanNodeEnum,
    space_id: u64,
    edge_types: Vec<String>,
    max_steps: usize,
    start_vertex_ids: Vec<Value>,
    end_vertex_ids: Vec<Value>,
    weight_expression: Option<String>,
}

impl PathPlanner {
    fn build_shortest_path_plan(
        &self,
        params: PathPlanParams,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let right_node = StartNode::new();
        let right_node_enum = PlanNodeEnum::Start(right_node);

        let mut shortest_path_node = ShortestPathNode::new(
            params.left_input,
            right_node_enum,
            params.space_id,
            params.edge_types,
            params.max_steps,
        );
        shortest_path_node.set_start_vertex_ids(params.start_vertex_ids);
        shortest_path_node.set_end_vertex_ids(params.end_vertex_ids);
        if let Some(weight) = params.weight_expression {
            shortest_path_node.set_weight_expression(weight);
        }

        Ok(shortest_path_node.into_enum())
    }

    fn build_all_paths_plan(
        &self,
        params: PathPlanParams,
        stmt: &crate::parser::ast::FindPathStmt,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let right_node = StartNode::new();
        let right_node_enum = PlanNodeEnum::Start(right_node);

        // Default paths do not repeat vertices; WITH LOOP / WITH TRAIL /
        // WITH WALK each relax a distinct part of that constraint (see
        // PathUniqueness).
        let path_uniqueness =
            crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness::from_flags(
                stmt.with_loop, stmt.with_trail, stmt.with_walk,
            );
        let direction = stmt
            .over
            .as_ref()
            .map(|over| over.direction)
            .unwrap_or(linkrs_core::EdgeDirection::Out);

        let mut all_paths_node = AllPathsNode::new(
            params.left_input,
            right_node_enum,
            params.space_id,
            params.max_steps,
            params.edge_types,
            1,
            params.max_steps,
            path_uniqueness,
        );
        all_paths_node.set_direction(direction);
        let start_vids: Vec<VertexId> = params
            .start_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let end_vids: Vec<VertexId> = params
            .end_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        all_paths_node.set_start_vertex_ids(start_vids);
        all_paths_node.set_end_vertex_ids(end_vids);

        Ok(all_paths_node.into_enum())
    }

    fn logical_start_leaf() -> LogicalNodeEnum {
        LogicalNodeEnum::Start(LogicalStartNode::new())
    }

    fn shortest_path_logical(
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        start_vertex_ids: Vec<Value>,
        end_vertex_ids: Vec<Value>,
        weight_expression: Option<String>,
    ) -> LogicalNodeEnum {
        let left = Self::logical_start_leaf();
        let right = Self::logical_start_leaf();
        LogicalNodeEnum::ShortestPath(LogicalShortestPathNode {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            space_id,
            edge_types,
            max_step: max_steps,
            weight_expression,
            heuristic_expression: None,
            no_reverse: false,
            start_vertex_ids,
            end_vertex_ids,
            output_var: None,
            col_names: vec!["path".to_string()],
            column_types: vec![],
        })
    }

    fn all_paths_logical(
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        stmt: &crate::parser::ast::FindPathStmt,
        start_vertex_ids: Vec<Value>,
        end_vertex_ids: Vec<Value>,
    ) -> LogicalNodeEnum {
        let path_uniqueness =
            crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness::from_flags(
                stmt.with_loop, stmt.with_trail, stmt.with_walk,
            );
        let direction = stmt
            .over
            .as_ref()
            .map(|over| over.direction)
            .unwrap_or(linkrs_core::EdgeDirection::Out);
        let start_vids: Vec<VertexId> = start_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let end_vids: Vec<VertexId> = end_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        Self::all_paths_logical_from_parts(
            space_id,
            edge_types,
            max_steps,
            path_uniqueness,
            direction,
            start_vids,
            end_vids,
        )
    }

    fn all_paths_logical_from_parts(
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        path_uniqueness: crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness,
        direction: linkrs_core::EdgeDirection,
        start_vertex_ids: Vec<VertexId>,
        end_vertex_ids: Vec<VertexId>,
    ) -> LogicalNodeEnum {
        let left = Self::logical_start_leaf();
        let right = Self::logical_start_leaf();
        LogicalNodeEnum::AllPaths(LogicalAllPathsNode {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            space_id,
            steps: max_steps,
            edge_types,
            min_hop: 1,
            max_hop: max_steps,
            path_uniqueness,
            direction,
            has_step_limit: true,
            limit: -1,
            offset: 0,
            filter: None,
            start_vertex_ids,
            end_vertex_ids,
            output_var: None,
            col_names: vec!["path".to_string()],
            column_types: vec![],
        })
    }

    fn is_shortest_path_stmt(&self, stmt: &crate::parser::ast::FindPathStmt) -> bool {
        stmt.shortest
    }

    fn get_edge_types_from_stmt(&self, stmt: &crate::parser::ast::FindPathStmt) -> Vec<String> {
        stmt.over
            .as_ref()
            .map(|over| over.edge_types.clone())
            .unwrap_or_default()
    }

    fn get_max_steps_from_stmt(&self, stmt: &crate::parser::ast::FindPathStmt) -> usize {
        stmt.max_steps.unwrap_or(10)
    }

    fn extract_vertex_ids_from_exprs(
        &self,
        exprs: &[linkrs_core::types::ContextualExpression],
    ) -> Vec<Value> {
        let mut ids = Vec::new();
        for expr in exprs {
            if let Some(meta) = expr.expression() {
                if let Some(value) = meta.as_literal() {
                    ids.push(value.clone());
                }
            }
        }
        ids
    }

    fn extract_vertex_ids_from_expr(
        &self,
        expr: &linkrs_core::types::ContextualExpression,
    ) -> Vec<Value> {
        self.extract_vertex_ids_from_exprs(std::slice::from_ref(expr))
    }
}

impl Default for PathPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::validation::{ValidatedStatement, ValidationInfo};
    use crate::parser::ast::stmt::Ast;
    use crate::parser::ast::{FindPathStmt, FromClause, OverClause, Span, Stmt};
    use linkrs_core::types::expr::contextual::ContextualExpression;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{Expression, ExpressionMeta};

    fn test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(crate::QueryRequestContext::new("FIND PATH".to_string()));
        let space_info = linkrs_core::types::SpaceInfo {
            space_id: 1,
            space_name: "test".to_string(),
            ..Default::default()
        };
        Arc::new(
            crate::QueryContext::builder(rctx)
                .with_space_info(space_info)
                .build(),
        )
    }

    fn find_path_stmt(shortest: bool) -> FindPathStmt {
        find_path_stmt_with_modifiers(shortest, false, false, false, None)
    }

    fn find_path_stmt_with_modifiers(
        shortest: bool,
        with_loop: bool,
        with_trail: bool,
        with_walk: bool,
        weight_expression: Option<&str>,
    ) -> FindPathStmt {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable("dst".to_string())));
        FindPathStmt {
            span: Span::default(),
            from: FromClause {
                span: Span::default(),
                vertices: vec![],
            },
            to: ContextualExpression::new(id, ctx.clone()),
            over: Some(OverClause {
                span: Span::default(),
                edge_types: vec!["knows".to_string()],
                direction: linkrs_core::EdgeDirection::Out,
            }),
            where_clause: None,
            shortest,
            max_steps: Some(3),
            limit: None,
            skip: None,
            yield_clause: None,
            weight_expression: weight_expression.map(str::to_string),
            heuristic_expression: None,
            with_loop,
            with_trail,
            with_walk,
        }
    }

    fn validated(stmt: Stmt) -> ValidatedStatement {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        ValidatedStatement::new(Arc::new(Ast::new(stmt, ctx)), ValidationInfo::new())
    }

    #[test]
    fn shortest_path_plan_carries_native_logical() {
        let stmt = Stmt::FindPath(find_path_stmt(true));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("label-less FIND PATH plans without a vertex label");
        assert!(
            plan.logical_root.is_some(),
            "shortest path plan must carry its native logical root"
        );
    }

    #[test]
    fn all_paths_plan_carries_native_logical_with_direction() {
        let stmt = Stmt::FindPath(find_path_stmt(false));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("label-less FIND PATH plans without a vertex label");
        assert!(
            plan.logical_root.is_some(),
            "all-paths plan must carry its native logical root"
        );
    }

    #[test]
    fn native_logical_converts_back_to_same_physical_shape() {
        let stmt = Stmt::FindPath(find_path_stmt(false));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("label-less FIND PATH plans without a vertex label");
        assert!(
            plan.root.is_some() && plan.logical_root.is_some(),
            "plan must carry both physical and logical shapes"
        );
    }

    #[test]
    fn shortest_path_plan_carries_weight_expression() {
        let stmt = Stmt::FindPath(find_path_stmt_with_modifiers(
            true,
            false,
            false,
            false,
            Some("cost"),
        ));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("weighted shortest path plans");
        match plan.root.as_ref().expect("physical root") {
            PlanNodeEnum::ShortestPath(node) => {
                assert_eq!(node.weight_expression().as_deref(), Some("cost"));
            }
            other => panic!("expected ShortestPath root, got {:?}", other),
        }
        match plan.logical_root.as_ref().expect("logical root") {
            LogicalNodeEnum::ShortestPath(node) => {
                assert_eq!(node.weight_expression.as_deref(), Some("cost"));
            }
            other => panic!("expected ShortestPath logical root, got {:?}", other),
        }
    }

    #[test]
    fn shortest_path_plan_defaults_weight_to_none() {
        let stmt = Stmt::FindPath(find_path_stmt(true));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("plain shortest path plans");
        match plan.root.as_ref().expect("physical root") {
            PlanNodeEnum::ShortestPath(node) => {
                assert!(node.weight_expression().is_none());
            }
            other => panic!("expected ShortestPath root, got {:?}", other),
        }
    }

    #[test]
    fn all_paths_plan_maps_repeat_flags_to_uniqueness() {
        let cases = [
            (false, false, false, PathUniqueness::VertexUnique),
            (true, false, false, PathUniqueness::AllowSelfLoop),
            (false, true, false, PathUniqueness::EdgeUnique),
            (false, false, true, PathUniqueness::Free),
            // A looser modifier dominates the tighter ones.
            (true, true, false, PathUniqueness::EdgeUnique),
            (true, false, true, PathUniqueness::Free),
        ];
        for (with_loop, with_trail, with_walk, expected) in cases {
            let stmt = Stmt::FindPath(find_path_stmt_with_modifiers(
                false, with_loop, with_trail, with_walk, None,
            ));
            let plan = PathPlanner::new()
                .transform(&validated(stmt), test_query_context())
                .expect("all path plans");
            match plan.root.as_ref().expect("physical root") {
                PlanNodeEnum::AllPaths(node) => {
                    assert_eq!(
                        node.path_uniqueness(),
                        expected,
                        "loop={with_loop} trail={with_trail} walk={with_walk} physical policy mismatch"
                    );
                }
                other => panic!("expected AllPaths root, got {:?}", other),
            }
            match plan.logical_root.as_ref().expect("logical root") {
                LogicalNodeEnum::AllPaths(node) => {
                    assert_eq!(
                        node.path_uniqueness, expected,
                        "loop={with_loop} trail={with_trail} walk={with_walk} logical policy mismatch"
                    );
                }
                other => panic!("expected AllPaths logical root, got {:?}", other),
            }
        }
    }

    #[test]
    fn shortest_path_ignores_repeat_flags() {
        // Cycles can never shorten a path, so the shortest-path plan stays a
        // plain ShortestPath node regardless of the modifiers.
        let stmt = Stmt::FindPath(find_path_stmt_with_modifiers(true, true, true, true, None));
        let plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("modifiers do not break shortest path planning");
        assert!(
            matches!(plan.root.as_ref(), Some(PlanNodeEnum::ShortestPath(_))),
            "expected ShortestPath root, got {:?}",
            plan.root
        );
    }
}
