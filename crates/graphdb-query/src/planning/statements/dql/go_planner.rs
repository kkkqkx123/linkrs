//! GO Statement Planner
//! Planning for handling Nebula GO queries
//!
//! ## Improvement Notes
//!
//! Implement the complete logic for filtering expressions.
//! Improving the handling of JOIN operations
//! - Add support for attribute projection.

use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::{ContextualExpression, EdgeDirection};
use crate::parser::ast::{GoStmt, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalDedupNode, LogicalFilterNode, LogicalProjectNode,
};
use crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use std::sync::Arc;

/// GO Query Planner
/// Responsible for converting GO statements into execution plans.
#[derive(Debug, Clone)]
pub struct GoPlanner {}

impl GoPlanner {
    /// Create a new GO planner.
    pub fn new() -> Self {
        Self {}
    }
}

impl Planner for GoPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let space_id = qctx.space_id().unwrap_or(1);

        let go_stmt = match validated.stmt() {
            Stmt::Go(go_stmt) => go_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "GoPlanner requires Go statements".to_string(),
                ));
            }
        };

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // 1. Check the optimization suggestions.
        for hint in &validation_info.optimization_hints {
            log::debug!("GO Optimization Tip: {:?}", hint);
        }

        // 2. Use the path analysis information
        for path_analysis in &validation_info.path_analysis {
            if path_analysis.edge_count > 5 {
                log::warn!(
                    "The GO path contains {} edges, which may affect performance.",
                    path_analysis.edge_count
                );
            }
        }

        // 3. Use semantic information
        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("GO referenced edge type: {:?}", referenced_edges);
        }

        // Handle FROM clause - extract source vertex IDs
        let from_vertices = &go_stmt.from.vertices;
        if from_vertices.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "GO statement must have FROM clause".to_string(),
            ));
        }

        // Check if the first from expression is a literal (vertex ID)
        let first_from = &from_vertices[0];
        let (use_start_node, from_var) = if first_from.is_literal() {
            // If it's a literal like "1", we need to create a variable in context
            // Use StartNode as the tail and set the variable in execution context
            (true, "v".to_string())
        } else if let Some(var_name) = first_from.as_variable() {
            // If it's already a variable, use ArgumentNode
            (false, var_name.clone())
        } else {
            // For other expressions, use ArgumentNode with a default variable name
            (false, "v".to_string())
        };

        // Create the tail node — this becomes the input to ExpandAllNode.
        let tail_logical = if use_start_node {
            LogicalNodeEnum::Start(LogicalStartNode::new())
        } else {
            LogicalNodeEnum::Argument(LogicalArgumentNode {
                id: next_node_id(),
                var: from_var.clone(),
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            })
        };

        let (direction_str, edge_types) = if let Some(over_clause) = &go_stmt.over {
            let direction_str = match over_clause.direction {
                EdgeDirection::Out => "out",
                EdgeDirection::In => "in",
                EdgeDirection::Both => "both",
            };
            (direction_str, over_clause.edge_types.clone())
        } else {
            ("both", vec![])
        };

        // Set step_limit based on GO statement steps
        let step_limit = match go_stmt.steps {
            crate::parser::ast::Steps::Fixed(n) => n as u32,
            crate::parser::ast::Steps::Range { min: _, max } => max as u32,
            crate::parser::ast::Steps::Variable(_) => 1,
        };

        // Set column names to match ExpandAll's output format: [src, edge, dst]
        // Also add edge type name as variable for accessing edge properties
        let mut col_names = vec!["src".to_string(), "edge".to_string(), "dst".to_string()];
        if edge_types.len() == 1 {
            col_names.push(edge_types[0].clone());
        }

        // Set src_vids from FROM clause if they are literals
        let src_vids: Vec<graphdb_core::Value> = if use_start_node {
            from_vertices
                .iter()
                .filter_map(|expr| expr.as_literal())
                .collect()
        } else {
            vec![]
        };

        // Build a pure logical tree natively (GO statements have no
        // planner-level physical choices) and convert it to the physical plan
        // exactly once at the plan exit via `SubPlan::from_logical_root`.
        let mut logical_root = LogicalNodeEnum::ExpandAll(LogicalExpandAllNode {
            id: next_node_id(),
            deps: vec![tail_logical],
            space_id,
            edge_types: edge_types.clone(),
            direction: direction_str.to_string(),
            any_edge_type: false,
            step_limit: Some(step_limit),
            step_limits: None,
            join_input: false,
            sample: false,
            edge_props: vec![],
            vertex_props: vec![],
            filter: None,
            src_vids,
            include_empty_paths: false,
            input_var: None,
            output_var: None,
            col_names,
            column_types: vec![],
        });

        if let Some(ref condition) = go_stmt.where_clause {
            logical_root = LogicalNodeEnum::Filter(LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                condition: condition.clone(),
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            });
        }

        let project_columns = Self::build_yield_columns(go_stmt, validated.expr_context())?;
        logical_root = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(logical_root)),
            deps: vec![],
            columns: project_columns.clone(),
            output_var: None,
            col_names: project_columns
                .iter()
                .map(|col| col.alias.clone())
                .collect(),
            column_types: vec![],
        });

        if step_limit > 1 {
            logical_root = LogicalNodeEnum::Dedup(LogicalDedupNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                deps: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            });
        }

        Ok(SubPlan::from_logical_root(logical_root))
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Go(_))
    }
}

impl GoPlanner {
    /// Create the YIELD column
    fn build_yield_columns(
        go_stmt: &GoStmt,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<Vec<graphdb_core::YieldColumn>, PlannerError> {
        let mut columns = Vec::new();

        if let Some(ref yield_clause) = go_stmt.yield_clause {
            for item in &yield_clause.items {
                columns.push(graphdb_core::YieldColumn {
                    expression: item.expression.clone(),
                    alias: item.alias.clone().unwrap_or_default(),
                    is_matched: false,
                });
            }
        } else {
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::Expression::Variable("dst".to_string()),
            );
            let id = expr_context.register_expression(expr_meta);
            let ctx_expr = ContextualExpression::new(id, expr_context.clone());
            columns.push(graphdb_core::YieldColumn {
                expression: ctx_expr,
                alias: "dst".to_string(),
                is_matched: false,
            });

            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::Expression::Variable("edge".to_string()),
            );
            let id = expr_context.register_expression(expr_meta);
            let ctx_expr = ContextualExpression::new(id, expr_context.clone());
            columns.push(graphdb_core::YieldColumn {
                expression: ctx_expr,
                alias: "edge".to_string(),
                is_matched: false,
            });
        }

        if columns.is_empty() {
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::Expression::Variable("*".to_string()),
            );
            let id = expr_context.register_expression(expr_meta);
            let ctx_expr = ContextualExpression::new(id, expr_context.clone());
            columns.push(graphdb_core::YieldColumn {
                expression: ctx_expr,
                alias: "result".to_string(),
                is_matched: false,
            });
        }

        Ok(columns)
    }
}

impl Default for GoPlanner {
    fn default() -> Self {
        Self::new()
    }
}
