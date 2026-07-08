//! PATH Query Planner
//! Planning for handling Nebula PATH query requests
//!
//! ## Explanation of the improvements
//!
//! Implementing shortest path planning
//! Implement all path planning functions.
//! Support for the shortest path with weights
//! Improve the logic for path filtering.

use crate::core::types::VertexId;
use crate::core::Value;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::traversal::{AllPathsNode, ShortestPathNode};
use crate::query::planning::plan::core::PlanNode;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

pub use crate::query::planning::plan::core::nodes::{
    ArgumentNode, DedupNode, ExpandAllNode, FilterNode, GetNeighborsNode, ProjectNode, StartNode,
};
pub use crate::query::planning::plan::core::PlanNodeEnum;

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

        let root_node = if self.is_shortest_path_stmt(find_path_stmt) {
            self.build_shortest_path_plan(
                start_node_enum.clone(),
                space_id,
                edge_types,
                max_steps,
                start_vertex_ids,
                end_vertex_ids,
            )?
        } else {
            self.build_all_paths_plan(
                start_node_enum.clone(),
                space_id,
                edge_types,
                max_steps,
                start_vertex_ids,
                end_vertex_ids,
            )?
        };

        let sub_plan = SubPlan {
            root: Some(root_node),
            tail: Some(start_node_enum),
        };

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::FindPath(_))
    }
}

impl PathPlanner {
    fn build_shortest_path_plan(
        &self,
        left_input: PlanNodeEnum,
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        start_vertex_ids: Vec<Value>,
        end_vertex_ids: Vec<Value>,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let right_node = StartNode::new();
        let right_node_enum = PlanNodeEnum::Start(right_node);

        let mut shortest_path_node =
            ShortestPathNode::new(left_input, right_node_enum, space_id, edge_types, max_steps);
        shortest_path_node.set_start_vertex_ids(start_vertex_ids);
        shortest_path_node.set_end_vertex_ids(end_vertex_ids);

        Ok(shortest_path_node.into_enum())
    }

    fn build_all_paths_plan(
        &self,
        left_input: PlanNodeEnum,
        space_id: u64,
        edge_types: Vec<String>,
        max_steps: usize,
        start_vertex_ids: Vec<Value>,
        end_vertex_ids: Vec<Value>,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let right_node = StartNode::new();
        let right_node_enum = PlanNodeEnum::Start(right_node);

        let mut all_paths_node = AllPathsNode::new(
            left_input,
            right_node_enum,
            space_id,
            max_steps,
            edge_types,
            1,
            max_steps,
            false,
        );
        let start_vids: Vec<VertexId> = start_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let end_vids: Vec<VertexId> = end_vertex_ids
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        all_paths_node.set_start_vertex_ids(start_vids);
        all_paths_node.set_end_vertex_ids(end_vids);

        Ok(all_paths_node.into_enum())
    }

    fn is_shortest_path_stmt(&self, stmt: &crate::query::parser::ast::FindPathStmt) -> bool {
        stmt.shortest
    }

    fn get_edge_types_from_stmt(
        &self,
        stmt: &crate::query::parser::ast::FindPathStmt,
    ) -> Vec<String> {
        stmt.over
            .as_ref()
            .map(|over| over.edge_types.clone())
            .unwrap_or_default()
    }

    fn get_max_steps_from_stmt(&self, stmt: &crate::query::parser::ast::FindPathStmt) -> usize {
        stmt.max_steps.unwrap_or(10)
    }

    fn extract_vertex_ids_from_exprs(
        &self,
        exprs: &[crate::core::types::ContextualExpression],
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
        expr: &crate::core::types::ContextualExpression,
    ) -> Vec<Value> {
        self.extract_vertex_ids_from_exprs(std::slice::from_ref(expr))
    }
}

impl Default for PathPlanner {
    fn default() -> Self {
        Self::new()
    }
}
