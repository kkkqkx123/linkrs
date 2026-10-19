//! Merge Operation Planner
//!
//! Query planning for handling MERGE statements with ON MATCH and ON CREATE clause support.
//!
//! ## MERGE Semantics
//!
//! MERGE ensures that a pattern exists in the graph:
//! - If the pattern matches existing data -> execute ON MATCH actions (if any)
//! - If the pattern does not exist -> create new data and execute ON CREATE actions (if any)

use crate::binder::bound::{
    BoundAssignment, BoundMergePattern, BoundPatternVertex, BoundStatement,
};
use crate::binder::expr_converter::bound_expr_to_contextual;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    EdgeInsertInfo, TagInsertSpec, UpdateTargetType, VertexInsertInfo, VertexUpdateInfo,
};
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode;
use crate::planning::plan::logical::logical_nodes::dml::{
    LogicalInsertEdgesNode, LogicalInsertVerticesNode, LogicalUpdateNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::ExpressionMeta;
use linkrs_core::types::expr::FunctionArg;
use linkrs_core::{Expression, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Merge Operation Planner
/// Responsible for converting MERGE statements into execution plans.
#[derive(Debug, Clone)]
pub struct MergePlanner;

impl MergePlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn match_stmt(stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Merge(_))
    }

    /// Bound-statement counterpart for constant checking: after DML
    /// shape normalization every literal leaf reaches the planner as a
    /// `@__dml_N` parameter, so any `ParameterRef` is accepted here while
    /// row variables, property references and subqueries are rejected.
    fn is_constant_bound_expression(expr: &crate::binder::bound::BoundExpression) -> bool {
        use crate::binder::bound::BoundExpression;
        match expr {
            BoundExpression::Literal(..)
            | BoundExpression::ParameterRef(..)
            | BoundExpression::SessionVariable(..) => true,
            BoundExpression::List(items, _) => items.iter().all(Self::is_constant_bound_expression),
            BoundExpression::Map(entries, _) => entries
                .iter()
                .all(|(_, v)| Self::is_constant_bound_expression(v)),
            BoundExpression::BinaryOp { left, right, .. } => {
                Self::is_constant_bound_expression(left)
                    && Self::is_constant_bound_expression(right)
            }
            BoundExpression::UnaryOp { operand, .. } => Self::is_constant_bound_expression(operand),
            BoundExpression::Cast { expr, .. } => Self::is_constant_bound_expression(expr),
            _ => false,
        }
    }

    fn create_vid_expression(
        &self,
        expr_context: &Arc<linkrs_core::types::expr::expression_context::ExpressionAnalysisContext>,
    ) -> Result<ContextualExpression, PlannerError> {
        let random_id = rand::random::<i64>().abs();
        let vid_meta = ExpressionMeta::new(Expression::Literal(Value::BigInt(random_id)));
        let vid_id = expr_context.register_expression(vid_meta);
        Ok(ContextualExpression::new(vid_id, expr_context.clone()))
    }

    fn create_exists_condition(
        &self,
        expr_context: &Arc<linkrs_core::types::expr::expression_context::ExpressionAnalysisContext>,
    ) -> Result<ContextualExpression, PlannerError> {
        let condition = Expression::Function {
            name: "exists".to_string(),
            args: vec![FunctionArg::Positional(Expression::Variable(
                "merged_vertex".to_string(),
            ))],
        };
        let meta = ExpressionMeta::new(condition);
        let id = expr_context.register_expression(meta);
        Ok(ContextualExpression::new(id, expr_context.clone()))
    }

    fn bound_vertex_to_info(
        &self,
        vertex: &BoundPatternVertex,
        space_name: String,
        expr_ctx: &Arc<ExpressionAnalysisContext>,
    ) -> Result<VertexInsertInfo, PlannerError> {
        let tag_name = vertex
            .labels
            .first()
            .ok_or_else(|| {
                PlannerError::PlanGenerationFailed(
                    "MERGE node pattern must have a label".to_string(),
                )
            })?
            .clone();

        let (prop_names, prop_values, vid_expr) = match &vertex.properties {
            Some(props) => {
                let mut names = Vec::new();
                let mut values = Vec::new();
                for (key, bound_expr) in props {
                    if !Self::is_constant_bound_expression(bound_expr) {
                        return Err(PlannerError::PlanGenerationFailed(format!(
                            "MERGE pattern property '{}' must be a constant expression; \
                             row variables, property references, and subqueries are not allowed",
                            key
                        )));
                    }
                    names.push(key.clone());
                    let ctx = bound_expr_to_contextual(bound_expr, expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;
                    values.push(ctx);
                }
                let vid_expr = if let Some(Expression::Literal(Value::Int(i))) =
                    values.first().and_then(|v| v.get_expression())
                {
                    let vid_meta = ExpressionMeta::new(Expression::Literal(Value::Int(i)));
                    let vid_id = expr_ctx.register_expression(vid_meta);
                    ContextualExpression::new(vid_id, expr_ctx.clone())
                } else {
                    self.create_vid_expression(expr_ctx)?
                };
                (names, values, vid_expr)
            }
            None => {
                let vid_expr = self.create_vid_expression(expr_ctx)?;
                (vec![], vec![], vid_expr)
            }
        };

        let tag_spec = TagInsertSpec {
            tag_name,
            prop_names,
        };
        Ok(VertexInsertInfo {
            space_name,
            tag: tag_spec,
            values: vec![(vid_expr, prop_values)],
            if_not_exists: true,
        })
    }

    fn bound_assignments_to_properties(
        assignments: &[BoundAssignment],
        expr_ctx: &Arc<ExpressionAnalysisContext>,
    ) -> Result<HashMap<String, ContextualExpression>, PlannerError> {
        let mut properties = HashMap::new();
        for assignment in assignments {
            let ctx = bound_expr_to_contextual(&assignment.value, expr_ctx)
                .map_err(PlannerError::PlanGenerationFailed)?;
            properties.insert(assignment.property.clone(), ctx);
        }
        Ok(properties)
    }

    fn build_update_info_from_bound(
        assignments: &[BoundAssignment],
        space_name: String,
        expr_ctx: &Arc<ExpressionAnalysisContext>,
    ) -> Result<VertexUpdateInfo, PlannerError> {
        let properties = Self::bound_assignments_to_properties(assignments, expr_ctx)?;

        let exists_expr = Expression::Literal(Value::Bool(true));
        let exists_meta = ExpressionMeta::new(exists_expr);
        let exists_id = expr_ctx.register_expression(exists_meta);
        let vid_expr = ContextualExpression::new(exists_id, expr_ctx.clone());

        Ok(VertexUpdateInfo {
            space_name,
            vertex_id: vid_expr,
            tag_name: None,
            properties,
            condition: None,
            is_upsert: false,
            replace_properties: assignments.iter().any(|a| a.is_map_overwrite),
        })
    }

    /// Build the standalone insert root for a node-pattern merge together
    /// with the vertex-id expression the insert will use.
    ///
    /// Shared by the bare-merge plan path and the conditional-merge
    /// orchestration so both compute the same insert values and vid.
    pub(crate) fn build_node_insert_parts(
        &self,
        vertex: &BoundPatternVertex,
        space_name: String,
        expr_ctx: &Arc<ExpressionAnalysisContext>,
    ) -> Result<(LogicalNodeEnum, ContextualExpression), PlannerError> {
        let vertex_info = self.bound_vertex_to_info(vertex, space_name, expr_ctx)?;
        let vid_expr = vertex_info
            .values
            .first()
            .map(|(vid, _)| vid.clone())
            .ok_or_else(|| {
                PlannerError::PlanGenerationFailed(
                    "MERGE node pattern produced no vertex id".to_string(),
                )
            })?;
        let insert = LogicalNodeEnum::InsertVertices(LogicalInsertVerticesNode {
            id: next_node_id(),
            info: vertex_info,
            output_var: None,
            col_names: vec!["inserted".to_string()],
            column_types: vec![],
        });
        Ok((insert, vid_expr))
    }

    /// Build the update root targeting one concrete vertex for a merge
    /// branch (`ON MATCH` on the probed vertex, `ON CREATE` on the vertex
    /// the create branch just inserted).
    ///
    /// Unlike the display-only `Select` branches, the vid and tag are fully
    /// resolved here so the update sink can locate its target row.
    pub(crate) fn build_node_update_root(
        assignments: &[BoundAssignment],
        space_name: String,
        tag_name: String,
        vid: Value,
        expr_ctx: &Arc<ExpressionAnalysisContext>,
    ) -> Result<LogicalNodeEnum, PlannerError> {
        let properties = Self::bound_assignments_to_properties(assignments, expr_ctx)?;
        let vid_meta = ExpressionMeta::new(Expression::Literal(vid));
        let vid_id = expr_ctx.register_expression(vid_meta);
        let info = VertexUpdateInfo {
            space_name,
            vertex_id: ContextualExpression::new(vid_id, expr_ctx.clone()),
            tag_name: Some(tag_name),
            properties,
            condition: None,
            is_upsert: false,
            replace_properties: assignments.iter().any(|a| a.is_map_overwrite),
        };
        Ok(LogicalNodeEnum::Update(LogicalUpdateNode {
            id: next_node_id(),
            info: UpdateTargetType::Vertex(info),
            output_var: None,
            col_names: vec!["updated".to_string()],
            column_types: vec![],
        }))
    }
}

impl Planner for MergePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let merge = match bound {
            BoundStatement::Merge(m) => m,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the MERGE".to_string(),
                ));
            }
        };

        // Unified entry for expression-level EXISTS / IN (mirrors
        // `transform`): the bound path must also reject subqueries in MERGE
        // pattern property values or ON MATCH / ON CREATE SET values at
        // planning time with the precise error.
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        match &merge.pattern {
            BoundMergePattern::Node(vertex) => {
                let has_on_match = !merge.on_match.is_empty();
                let has_on_create = !merge.on_create.is_empty();

                if !has_on_match && !has_on_create {
                    let (logical_root, _) =
                        self.build_node_insert_parts(vertex, space_name.clone(), &expr_ctx)?;
                    return Ok(SubPlan::from_logical_root(logical_root));
                }

                let condition = self.create_exists_condition(&expr_ctx)?;
                let mut select_node = LogicalSelectNode {
                    id: next_node_id(),
                    condition,
                    if_branch: None,
                    else_branch: None,
                    output_var: None,
                    col_names: vec![],
                    column_types: vec![],
                };

                if has_on_match {
                    let update_info = Self::build_update_info_from_bound(
                        &merge.on_match,
                        space_name.clone(),
                        &expr_ctx,
                    )?;
                    select_node.set_if_branch(LogicalNodeEnum::Update(LogicalUpdateNode {
                        id: next_node_id(),
                        info: UpdateTargetType::Vertex(update_info),
                        output_var: None,
                        col_names: vec!["updated".to_string()],
                        column_types: vec![],
                    }));
                }

                let mut current_node = {
                    let (insert, _) =
                        self.build_node_insert_parts(vertex, space_name.clone(), &expr_ctx)?;
                    insert
                };
                if has_on_create {
                    let update_info = Self::build_update_info_from_bound(
                        &merge.on_create,
                        space_name.clone(),
                        &expr_ctx,
                    )?;
                    current_node = LogicalNodeEnum::Update(LogicalUpdateNode {
                        id: next_node_id(),
                        info: UpdateTargetType::Vertex(update_info),
                        output_var: None,
                        col_names: vec!["updated".to_string()],
                        column_types: vec![],
                    });
                }
                select_node.set_else_branch(current_node);

                let logical_root = LogicalNodeEnum::Select(select_node);
                Ok(SubPlan::from_logical_root(logical_root))
            }
            BoundMergePattern::Edge { edge, .. } => {
                if !merge.on_match.is_empty() || !merge.on_create.is_empty() {
                    return Err(PlannerError::UnsupportedOperation(
                        "MERGE edge pattern with ON MATCH / ON CREATE is not supported".to_string(),
                    ));
                }
                let expr_ctx = Arc::new(
                    linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                );
                let edge_name = edge
                    .edge_types
                    .first()
                    .cloned()
                    .ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "MERGE edge pattern must have an edge type".to_string(),
                        )
                    })?;
                let (prop_names, prop_values) = if let Some(props) = &edge.properties {
                    let mut names = Vec::new();
                    let mut values = Vec::new();
                    for (key, bexpr) in props {
                        names.push(key.clone());
                        let ctx_expr = crate::binder::expr_converter::bound_expr_to_contextual(
                            bexpr, &expr_ctx,
                        )
                        .map_err(PlannerError::PlanGenerationFailed)?;
                        values.push(ctx_expr);
                    }
                    (names, values)
                } else {
                    (Vec::new(), Vec::new())
                };
                let src_expr = self.create_vid_expression(&expr_ctx)?;
                let dst_expr = self.create_vid_expression(&expr_ctx)?;
                let edge_info = EdgeInsertInfo {
                    space_name: space_name.clone(),
                    edge_name,
                    prop_names,
                    edges: vec![(src_expr, dst_expr, None, prop_values)],
                    if_not_exists: true,
                };
                let logical_root = LogicalNodeEnum::InsertEdges(LogicalInsertEdgesNode {
                    id: next_node_id(),
                    info: edge_info,
                    output_var: None,
                    col_names: vec!["inserted".to_string()],
                    column_types: vec![],
                });
                return Ok(SubPlan::from_logical_root(logical_root));
            }
        }
    }


}

impl Default for MergePlanner {
    fn default() -> Self {
        Self::new()
    }
}
