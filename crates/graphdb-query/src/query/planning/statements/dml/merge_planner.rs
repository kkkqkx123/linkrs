//! Merge Operation Planner
//!
//! Query planning for handling MERGE statements with ON MATCH and ON CREATE clause support.
//!
//! ## MERGE Semantics
//!
//! MERGE ensures that a pattern exists in the graph:
//! - If the pattern matches existing data -> execute ON MATCH actions (if any)
//! - If the pattern does not exist -> create new data and execute ON CREATE actions (if any)

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::ExpressionMeta;
use crate::core::{Expression, Value};
use crate::query::parser::ast::{MergeStmt, Pattern, SetClause, Stmt};
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::{
    ArgumentNode, EdgeInsertInfo, InsertEdgesNode, InsertVerticesNode, SelectNode, TagInsertSpec,
    UpdateNode, UpdateTargetType, VertexInsertInfo, VertexUpdateInfo,
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;

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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
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
        expr_context: &Arc<crate::query::validator::context::ExpressionAnalysisContext>,
    ) -> Result<ContextualExpression, PlannerError> {
        let condition = Expression::Function {
            name: "exists".to_string(),
            args: vec![Expression::Variable("merged_vertex".to_string())],
        };
        let meta = ExpressionMeta::new(condition);
        let id = expr_context.register_expression(meta);
        Ok(ContextualExpression::new(id, expr_context.clone()))
    }
}

impl Planner for MergePlanner {
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
