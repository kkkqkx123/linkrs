//! The FETCH VERTICES query planner
//! Planning for the FETCH VERTICES query

use crate::parser::ast::{FetchTarget, Stmt};
use crate::planning::plan::core::common::TagProp;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    ArgumentNode, GetVerticesNode, PlanNodeEnum, ProjectNode,
};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use std::sync::Arc;

/// The FETCH VERTICES query planner
/// Responsible for converting the FETCH VERTICES query into an execution plan.
#[derive(Debug, Clone)]
pub struct FetchVerticesPlanner;

impl FetchVerticesPlanner {
    /// Create a new FETCH VERTICES planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for FetchVerticesPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let fetch_stmt = match validated.stmt() {
            Stmt::Fetch(fetch_stmt) => fetch_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchVerticesPlanner requires the Fetch statement.".to_string(),
                ));
            }
        };

        // Check whether it is a FETCH VERTICES operation.
        let (tag_name, ids, properties) = match &fetch_stmt.target {
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties,
                ..
            } => (tag_name, ids, properties),
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "FetchVerticesPlanner requires the FETCH VERTICES statement.".to_string(),
                ));
            }
        };

        // Get space_id and space_name from query context
        let space_id = qctx.space_id().unwrap_or(0);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        // Convert ids to comma-separated string for GetVerticesNode
        let ids_str = ids
            .iter()
            .map(|ctx_expr| {
                if let Some(expr_meta) = ctx_expr.expression() {
                    let expr = expr_meta.inner();
                    match expr {
                        graphdb_core::Expression::Literal(graphdb_core::Value::Int(i)) => {
                            i.to_string()
                        }
                        graphdb_core::Expression::Literal(graphdb_core::Value::BigInt(i)) => {
                            i.to_string()
                        }
                        graphdb_core::Expression::Literal(graphdb_core::Value::String(s)) => {
                            s.to_string()
                        }
                        _ => expr.to_string(),
                    }
                } else {
                    String::new()
                }
            })
            .collect::<Vec<_>>()
            .join(",");

        let var_name = "v";
        let entity_var = tag_name.clone().unwrap_or_else(|| "vertex".to_string());

        // 1. Create a parameter node to provide the vertex IDs.
        let mut arg_node = ArgumentNode::new(next_node_id(), var_name);
        arg_node.set_col_names(vec!["vid".to_string()]);
        arg_node.set_output_var("vertex_ids".to_string());

        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        // 2. Create GetVerticesNode with proper space_id and space_name
        // Use actual ids_str instead of var_name to pass vertex IDs directly
        let mut get_vertices_node = GetVerticesNode::new(space_id, &space_name, &ids_str);
        get_vertices_node.add_dependency(arg_node_enum.clone());
        get_vertices_node.set_output_var("fetched_vertices".to_string());
        get_vertices_node.set_col_names(vec![entity_var.clone()]);

        // Set the tag attributes (obtained from the properties field)
        let tag_props = if let Some(props) = properties {
            vec![TagProp::new("default", props.clone())]
        } else {
            vec![]
        };
        get_vertices_node.set_tag_props(tag_props);

        let get_vertices_node_enum = PlanNodeEnum::GetVertices(get_vertices_node);

        // 3. Apply the YIELD clause as a projection when present; without one
        // the fetched vertices are returned directly.
        let root = if let Some(ref yield_clause) = fetch_stmt.yield_clause {
            let mut columns = Vec::new();
            for item in &yield_clause.items {
                columns.push(graphdb_core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
            PlanNodeEnum::Project(ProjectNode::new(get_vertices_node_enum.clone(), columns)?)
        } else {
            get_vertices_node_enum.clone()
        };

        // 4. Create a SubPlan
        let sub_plan = SubPlan::new(Some(root), Some(arg_node_enum));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Fetch(fetch_stmt) => {
                matches!(&fetch_stmt.target, FetchTarget::Vertices { .. })
            }
            _ => false,
        }
    }
}

impl Default for FetchVerticesPlanner {
    fn default() -> Self {
        Self::new()
    }
}
