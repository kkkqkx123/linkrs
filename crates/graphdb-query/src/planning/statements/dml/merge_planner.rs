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
use crate::parser::ast::{MergeStmt, Pattern, SetClause, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    ArgumentNode, EdgeInsertInfo, InsertEdgesNode, InsertVerticesNode, SelectNode, TagInsertSpec,
    UpdateNode, UpdateTargetType, VertexInsertInfo, VertexUpdateInfo,
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::{Expression, Value};
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

    fn extract_merge_stmt(&self, stmt: &Stmt) -> Result<MergeStmt, PlannerError> {
        match stmt {
            Stmt::Merge(merge_stmt) => Ok(merge_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the MERGE".to_string(),
            )),
        }
    }

    fn pattern_to_vertex_info(
        &self,
        pattern: &Pattern,
        space_name: String,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<VertexInsertInfo, PlannerError> {
        match pattern {
            Pattern::Node(node_pattern) => {
                let tag_name = node_pattern
                    .labels
                    .first()
                    .ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "MERGE node pattern must have a label".to_string(),
                        )
                    })?
                    .clone();

                let (prop_names, prop_values, vid_expr) =
                    if let Some(props_expr) = &node_pattern.properties {
                        self.extract_properties_and_vid(props_expr, expr_context)?
                    } else {
                        let vid_expr = self.create_vid_expression(expr_context)?;
                        (vec![], vec![], vid_expr)
                    };

                let tag_spec = TagInsertSpec {
                    tag_name,
                    prop_names,
                };

                Ok(VertexInsertInfo {
                    space_name,
                    tags: vec![tag_spec],
                    values: vec![(vid_expr, vec![prop_values])],
                    if_not_exists: true,
                })
            }
            _ => Err(PlannerError::PlanGenerationFailed(
                "MERGE currently only supports node patterns".to_string(),
            )),
        }
    }

    fn pattern_to_edge_info(
        &self,
        pattern: &Pattern,
        space_name: String,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<EdgeInsertInfo, PlannerError> {
        match pattern {
            Pattern::Edge(edge_pattern) => {
                let edge_name = edge_pattern
                    .edge_types
                    .first()
                    .ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "MERGE edge pattern must have an edge type".to_string(),
                        )
                    })?
                    .clone();

                let (prop_names, prop_values) = if let Some(props_expr) = &edge_pattern.properties {
                    if let Some(Expression::Map(entries)) = props_expr.get_expression() {
                        let mut names = Vec::new();
                        let mut values = Vec::new();
                        for (key, value) in entries {
                            names.push(key.clone());
                            let value_meta = ExpressionMeta::new(value.clone());
                            let value_id = expr_context.register_expression(value_meta);
                            let ctx_value =
                                ContextualExpression::new(value_id, expr_context.clone());
                            values.push(ctx_value);
                        }
                        (names, values)
                    } else {
                        (vec![], vec![])
                    }
                } else {
                    (vec![], vec![])
                };

                let src_expr = self.create_vid_expression(expr_context)?;
                let dst_expr = self.create_vid_expression(expr_context)?;

                Ok(EdgeInsertInfo {
                    space_name,
                    edge_name,
                    prop_names,
                    edges: vec![(src_expr, dst_expr, None, prop_values)],
                    if_not_exists: true,
                })
            }
            _ => Err(PlannerError::PlanGenerationFailed(
                "pattern is not an edge pattern".to_string(),
            )),
        }
    }

    fn is_edge_pattern(&self, pattern: &Pattern) -> bool {
        matches!(pattern, Pattern::Edge(_))
    }

    fn extract_properties_and_vid(
        &self,
        props_expr: &ContextualExpression,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<(Vec<String>, Vec<ContextualExpression>, ContextualExpression), PlannerError> {
        if let Some(Expression::Map(entries)) = props_expr.get_expression() {
            let mut prop_names = Vec::new();
            let mut prop_values = Vec::new();

            for (key, value) in entries {
                prop_names.push(key.clone());

                let value_meta = ExpressionMeta::new(value.clone());
                let value_id = expr_context.register_expression(value_meta);
                let ctx_value = ContextualExpression::new(value_id, expr_context.clone());
                prop_values.push(ctx_value);
            }

            let vid_expr = if let Some(Expression::Literal(Value::Int(i))) =
                prop_values.first().and_then(|v| v.get_expression())
            {
                let vid_meta = ExpressionMeta::new(Expression::Literal(Value::Int(i)));
                let vid_id = expr_context.register_expression(vid_meta);
                ContextualExpression::new(vid_id, expr_context.clone())
            } else {
                self.create_vid_expression(expr_context)?
            };

            Ok((prop_names, prop_values, vid_expr))
        } else {
            let vid_expr = self.create_vid_expression(expr_context)?;
            Ok((vec![], vec![], vid_expr))
        }
    }

    fn create_vid_expression(
        &self,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<ContextualExpression, PlannerError> {
        let random_id = rand::random::<i64>().abs();
        let vid_meta = ExpressionMeta::new(Expression::Literal(Value::BigInt(random_id)));
        let vid_id = expr_context.register_expression(vid_meta);
        Ok(ContextualExpression::new(vid_id, expr_context.clone()))
    }

    fn build_update_info(
        &self,
        set_clause: &SetClause,
        space_name: String,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<VertexUpdateInfo, PlannerError> {
        let mut properties = HashMap::new();

        for assignment in &set_clause.assignments {
            properties.insert(assignment.property.clone(), assignment.value.clone());
        }

        let exists_expr = Expression::Literal(Value::Bool(true));
        let exists_meta = ExpressionMeta::new(exists_expr);
        let exists_id = expr_context.register_expression(exists_meta);
        let vid_expr = ContextualExpression::new(exists_id, expr_context.clone());

        Ok(VertexUpdateInfo {
            space_name,
            vertex_id: vid_expr,
            tag_name: None,
            properties,
            condition: None,
            is_upsert: false,
        })
    }

    fn build_on_match_branch(
        &self,
        on_match: &SetClause,
        space_name: String,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let update_info = self.build_update_info(on_match, space_name, expr_context)?;
        let update_node = UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(update_info));
        Ok(PlanNodeEnum::Update(update_node))
    }

    fn build_on_create_branch(
        &self,
        vertex_info: VertexInsertInfo,
        on_create: Option<&SetClause>,
        space_name: String,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let insert_node = InsertVerticesNode::new(next_node_id(), vertex_info);
        let mut current_node = PlanNodeEnum::InsertVertices(insert_node);

        if let Some(set_clause) = on_create {
            let update_info = self.build_update_info(set_clause, space_name, expr_context)?;
            let update_node =
                UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(update_info));
            current_node = PlanNodeEnum::Update(update_node);
        }

        Ok(current_node)
    }

    fn create_exists_condition(
        &self,
        expr_context: &Arc<
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext,
        >,
    ) -> Result<ContextualExpression, PlannerError> {
        let condition = Expression::Function {
            name: "exists".to_string(),
            args: vec![Expression::Variable("merged_vertex".to_string())],
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
                    names.push(key.clone());
                    let ctx = bound_expr_to_contextual(bound_expr, expr_ctx)
                        .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
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
            tags: vec![tag_spec],
            values: vec![(vid_expr, vec![prop_values])],
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
                .map_err(|e| PlannerError::PlanGenerationFailed(e))?;
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
        })
    }
}

impl Planner for MergePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let merge = match bound {
            BoundStatement::Merge(m) => m,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the MERGE".to_string(),
                ));
            }
        };

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        match &merge.pattern {
            BoundMergePattern::Node(vertex) => {
                let vertex_info =
                    self.bound_vertex_to_info(vertex, space_name.clone(), &expr_ctx)?;

                let has_on_match = !merge.on_match.is_empty();
                let has_on_create = !merge.on_create.is_empty();

                if !has_on_match && !has_on_create {
                    let arg_node = ArgumentNode::new(next_node_id(), "merge_args");
                    let arg_node_enum = PlanNodeEnum::Argument(arg_node);
                    let insert_node = InsertVerticesNode::new(next_node_id(), vertex_info);
                    let insert_node_enum = PlanNodeEnum::InsertVertices(insert_node);
                    return Ok(SubPlan::new(
                        Some(insert_node_enum),
                        Some(arg_node_enum),
                    ));
                }

                let arg_node = ArgumentNode::new(next_node_id(), "merge_args");
                let arg_node_enum = PlanNodeEnum::Argument(arg_node);
                let condition = self.create_exists_condition(&expr_ctx)?;
                let mut select_node = SelectNode::new(next_node_id(), condition);

                if has_on_match {
                    let update_info =
                        Self::build_update_info_from_bound(&merge.on_match, space_name.clone(), &expr_ctx)?;
                    let update_node =
                        UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(update_info));
                    select_node.set_if_branch(PlanNodeEnum::Update(update_node));
                }

                let insert_node = InsertVerticesNode::new(next_node_id(), vertex_info);
                let mut current_node = PlanNodeEnum::InsertVertices(insert_node);
                if has_on_create {
                    let update_info =
                        Self::build_update_info_from_bound(&merge.on_create, space_name.clone(), &expr_ctx)?;
                    let update_node =
                        UpdateNode::new(next_node_id(), UpdateTargetType::Vertex(update_info));
                    current_node = PlanNodeEnum::Update(update_node);
                }
                select_node.set_else_branch(current_node);

                let select_node_enum = PlanNodeEnum::Select(select_node);
                Ok(SubPlan::new(Some(select_node_enum), Some(arg_node_enum)))
            }
            BoundMergePattern::Edge { .. } => {
                // Edge MERGE still delegates to AST-based transform
                self.transform(validated, qctx)
            }
        }
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let validation_info = &validated.validation_info;

        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("MERGE quoted tags: {:?}", referenced_tags);
        }

        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("MERGE references edge type: {:?}", referenced_edges);
        }

        let referenced_properties = &validation_info.semantic_info.referenced_properties;
        if !referenced_properties.is_empty() {
            log::debug!("MERGE Referenced Properties: {:?}", referenced_properties);
        }

        let merge_stmt = self.extract_merge_stmt(validated.stmt())?;

        // Unified entry for expression-level EXISTS / IN: subqueries in MERGE
        // pattern property values or ON MATCH / ON CREATE SET values are
        // rejected at planning time with a precise error.
        let check_space_id = qctx.space_id().unwrap_or(1);
        let check_space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        let pattern_props: Option<&ContextualExpression> = match &merge_stmt.pattern {
            Pattern::Node(node_pattern) => node_pattern.properties.as_ref(),
            Pattern::Edge(edge_pattern) => edge_pattern.properties.as_ref(),
            _ => None,
        };
        if let Some(props_expr) = pattern_props {
            if let Some(expr_meta) = props_expr.expression() {
                exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    &qctx,
                    check_space_id,
                    &check_space_name,
                    &outer_col_names,
                )?;
            }
        }
        for set_clause in [&merge_stmt.on_match, &merge_stmt.on_create]
            .into_iter()
            .flatten()
        {
            for assignment in &set_clause.assignments {
                if let Some(expr_meta) = assignment.value.expression() {
                    exists_planner::check_expression_subqueries(
                        expr_meta.inner(),
                        &qctx,
                        check_space_id,
                        &check_space_name,
                        &outer_col_names,
                    )?;
                }
            }
        }

        let is_edge = self.is_edge_pattern(&merge_stmt.pattern);

        if is_edge {
            let edge_info = self.pattern_to_edge_info(
                &merge_stmt.pattern,
                space_name.clone(),
                validated.expr_context(),
            )?;

            let insert_node = InsertEdgesNode::new(next_node_id(), edge_info);
            let insert_node_enum = PlanNodeEnum::InsertEdges(insert_node);
            let sub_plan = SubPlan::from_single_node(insert_node_enum);
            return Ok(sub_plan);
        }

        let vertex_info = self.pattern_to_vertex_info(
            &merge_stmt.pattern,
            space_name.clone(),
            validated.expr_context(),
        )?;

        let has_on_match = merge_stmt.on_match.is_some();
        let has_on_create = merge_stmt.on_create.is_some();

        if !has_on_match && !has_on_create {
            let arg_node = ArgumentNode::new(next_node_id(), "merge_args");
            let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

            let insert_node = InsertVerticesNode::new(next_node_id(), vertex_info);
            let insert_node_enum = PlanNodeEnum::InsertVertices(insert_node);

            let sub_plan = SubPlan::new(Some(insert_node_enum), Some(arg_node_enum));
            return Ok(sub_plan);
        }

        let arg_node = ArgumentNode::new(next_node_id(), "merge_args");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        let condition = self.create_exists_condition(validated.expr_context())?;
        let mut select_node = SelectNode::new(next_node_id(), condition);

        if let Some(ref on_match) = merge_stmt.on_match {
            let if_branch =
                self.build_on_match_branch(on_match, space_name.clone(), validated.expr_context())?;
            select_node.set_if_branch(if_branch);
        }

        let else_branch = self.build_on_create_branch(
            vertex_info,
            merge_stmt.on_create.as_ref(),
            space_name,
            validated.expr_context(),
        )?;
        select_node.set_else_branch(else_branch);

        let select_node_enum = PlanNodeEnum::Select(select_node);
        let sub_plan = SubPlan::new(Some(select_node_enum), Some(arg_node_enum));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        Self::match_stmt(stmt)
    }
}

impl Default for MergePlanner {
    fn default() -> Self {
        Self::new()
    }
}
