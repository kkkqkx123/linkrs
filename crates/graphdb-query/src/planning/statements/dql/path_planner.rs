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
use crate::planning::plan::core::nodes::traversal::{AllPathsNode, ShortestPathNode};
use crate::planning::plan::core::PlanNode;
use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
use crate::planning::plan::logical::logical_nodes::algorithm::{
    LogicalAllPathsNode, LogicalShortestPathNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::VertexId;
use graphdb_core::Value;
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
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let find_path_stmt = match validated.stmt() {
            Stmt::FindPath(find_path_stmt) => find_path_stmt,
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

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node);

        let edge_types = self.get_edge_types_from_stmt(find_path_stmt);
        let max_steps = self.get_max_steps_from_stmt(find_path_stmt);

        let start_vertex_ids = self.extract_vertex_ids_from_exprs(&find_path_stmt.from.vertices);
        let end_vertex_ids = self.extract_vertex_ids_from_expr(&find_path_stmt.to);

        let (root_node, logical_root) = if self.is_shortest_path_stmt(find_path_stmt) {
            let root = self.build_shortest_path_plan(PathPlanParams {
                left_input: start_node_enum.clone(),
                space_id,
                edge_types: edge_types.clone(),
                max_steps,
                start_vertex_ids: start_vertex_ids.clone(),
                end_vertex_ids: end_vertex_ids.clone(),
            })?;
            let logical = Self::shortest_path_logical(
                space_id,
                edge_types,
                max_steps,
                start_vertex_ids,
                end_vertex_ids,
            );
            (root, logical)
        } else {
            let root = self.build_all_paths_plan(
                PathPlanParams {
                    left_input: start_node_enum.clone(),
                    space_id,
                    edge_types: edge_types.clone(),
                    max_steps,
                    start_vertex_ids: start_vertex_ids.clone(),
                    end_vertex_ids: end_vertex_ids.clone(),
                },
                find_path_stmt,
            )?;
            let logical = Self::all_paths_logical(
                space_id,
                edge_types,
                max_steps,
                find_path_stmt,
                start_vertex_ids,
                end_vertex_ids,
            );
            (root, logical)
        };

        let sub_plan = SubPlan {
            root: Some(root_node),
            tail: Some(start_node_enum),
            logical_root: Some(logical_root),
        };

        Ok(sub_plan)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
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

        let (root_node, logical_root) = if find_path.shortest {
            let root = self.build_shortest_path_plan(PathPlanParams {
                left_input: start_node_enum.clone(),
                space_id,
                edge_types: edge_types.clone(),
                max_steps,
                start_vertex_ids: start_vertex_ids.clone(),
                end_vertex_ids: end_vertex_ids.clone(),
            })?;
            let logical = Self::shortest_path_logical(
                space_id,
                edge_types,
                max_steps,
                start_vertex_ids,
                end_vertex_ids,
            );
            (root, logical)
        } else {
            let direction = find_path
                .over
                .as_ref()
                .map(|(_, dir)| *dir)
                .unwrap_or(graphdb_core::EdgeDirection::Out);

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
                true,
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
                space_id, edge_types, max_steps, true, direction, start_vids, end_vids,
            );
            (all_paths_node.into_enum(), logical)
        };

        let sub_plan = SubPlan {
            root: Some(root_node),
            tail: Some(start_node_enum),
            logical_root: Some(logical_root),
        };

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::FindPath(_))
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

        Ok(shortest_path_node.into_enum())
    }

    fn build_all_paths_plan(
        &self,
        params: PathPlanParams,
        stmt: &crate::parser::ast::FindPathStmt,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let right_node = StartNode::new();
        let right_node_enum = PlanNodeEnum::Start(right_node);

        // By default paths must not repeat vertices; WITH LOOP/WITH CYCLE
        // relaxes that constraint.
        let acyclic = !(stmt.with_loop || stmt.with_cycle);
        let direction = stmt
            .over
            .as_ref()
            .map(|over| over.direction)
            .unwrap_or(graphdb_core::EdgeDirection::Out);

        let mut all_paths_node = AllPathsNode::new(
            params.left_input,
            right_node_enum,
            params.space_id,
            params.max_steps,
            params.edge_types,
            1,
            params.max_steps,
            acyclic,
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
    ) -> LogicalNodeEnum {
        let left = Self::logical_start_leaf();
        let right = Self::logical_start_leaf();
        LogicalNodeEnum::ShortestPath(LogicalShortestPathNode {
            id: next_node_id(),
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
            deps: vec![left, right],
            space_id,
            edge_types,
            max_step: max_steps,
            weight_expression: None,
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
        let acyclic = !(stmt.with_loop || stmt.with_cycle);
        let direction = stmt
            .over
            .as_ref()
            .map(|over| over.direction)
            .unwrap_or(graphdb_core::EdgeDirection::Out);
        let start_vids: Vec<VertexId> = start_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let end_vids: Vec<VertexId> = end_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        Self::all_paths_logical_from_parts(
            space_id, edge_types, max_steps, acyclic, direction, start_vids, end_vids,
        )
    }

    fn all_paths_logical_from_parts(
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        acyclic: bool,
        direction: graphdb_core::EdgeDirection,
        start_vertex_ids: Vec<VertexId>,
        end_vertex_ids: Vec<VertexId>,
    ) -> LogicalNodeEnum {
        let left = Self::logical_start_leaf();
        let right = Self::logical_start_leaf();
        LogicalNodeEnum::AllPaths(LogicalAllPathsNode {
            id: next_node_id(),
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
            deps: vec![left, right],
            space_id,
            steps: max_steps,
            edge_types,
            min_hop: 1,
            max_hop: max_steps,
            acyclic,
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
        exprs: &[graphdb_core::types::ContextualExpression],
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
        expr: &graphdb_core::types::ContextualExpression,
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
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{Expression, ExpressionMeta};

    fn test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(crate::QueryRequestContext::new("FIND PATH".to_string()));
        let space_info = graphdb_core::types::SpaceInfo {
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
                direction: graphdb_core::EdgeDirection::Out,
            }),
            where_clause: None,
            shortest,
            max_steps: Some(3),
            limit: None,
            skip: None,
            yield_clause: None,
            weight_expression: None,
            heuristic_expression: None,
            with_loop: false,
            with_cycle: false,
        }
    }

    fn validated(stmt: Stmt) -> ValidatedStatement {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        ValidatedStatement::new(Arc::new(Ast::new(stmt, ctx)), ValidationInfo::new())
    }

    #[test]
    fn shortest_path_plan_carries_native_logical() {
        let stmt = Stmt::FindPath(find_path_stmt(true));
        let sub_plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("path planning should succeed");
        assert!(matches!(sub_plan.root, Some(PlanNodeEnum::ShortestPath(_))));
        let Some(LogicalNodeEnum::ShortestPath(logical)) = sub_plan.logical_root else {
            panic!(
                "expected native logical ShortestPath, got {:?}",
                sub_plan.logical_root.as_ref().map(|n| n.type_name())
            );
        };
        assert_eq!(logical.max_step, 3);
        assert_eq!(logical.edge_types, vec!["knows".to_string()]);
        assert_eq!(logical.col_names, vec!["path".to_string()]);
    }

    #[test]
    fn all_paths_plan_carries_native_logical_with_direction() {
        let stmt = Stmt::FindPath(find_path_stmt(false));
        let sub_plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("path planning should succeed");
        let Some(PlanNodeEnum::AllPaths(physical)) = &sub_plan.root else {
            panic!("expected physical AllPaths");
        };
        let Some(LogicalNodeEnum::AllPaths(logical)) = sub_plan.logical_root else {
            panic!("expected native logical AllPaths");
        };
        assert_eq!(logical.direction, physical.direction());
        assert_eq!(logical.max_hop, 3);
        assert_eq!(logical.min_hop, 1);
        assert!(logical.acyclic);
        assert_eq!(logical.col_names, physical.col_names().to_vec());
    }

    #[test]
    fn native_logical_converts_back_to_same_physical_shape() {
        let stmt = Stmt::FindPath(find_path_stmt(false));
        let sub_plan = PathPlanner::new()
            .transform(&validated(stmt), test_query_context())
            .expect("path planning should succeed");
        let logical = sub_plan.logical_root.clone().expect("logical must exist");
        let physical = sub_plan.root.clone().expect("physical must exist");
        let reconverted = crate::planning::physical_planner::convert_logical_to_physical(logical);
        assert_eq!(
            std::mem::discriminant(&reconverted),
            std::mem::discriminant(&physical)
        );
        assert_eq!(reconverted.col_names(), physical.col_names());
    }
}
