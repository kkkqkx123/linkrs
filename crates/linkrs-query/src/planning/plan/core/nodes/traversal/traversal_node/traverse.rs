use std::sync::Arc;

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::node_id_generator::next_node_id;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::{ContextualExpression, EdgeDirection, SerializableExpression};
use linkrs_core::Expression;

define_plan_node_with_deps! {
    pub struct TraverseNode {
        space_id: u64,
        start_vids: String,
        end_vids: Option<String>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_steps: u32,
        max_steps: u32,
        edge_alias: Option<String>,
        vertex_alias: Option<String>,
        e_filter: Option<ContextualExpression>,
        e_filter_serializable: Option<Box<SerializableExpression>>,
        v_filter: Option<ContextualExpression>,
        v_filter_serializable: Option<Box<SerializableExpression>>,
        first_step_filter: Option<ContextualExpression>,
        first_step_filter_serializable: Option<Box<SerializableExpression>>,
        path_semantic: Option<crate::parser::ast::pattern::PathSemantic>,
        dst_tag: Option<String>,
    }
    enum: Traverse
    input: SingleInputNode
}

impl TraverseNode {
    pub fn new(space_id: u64, start_vids: &str, min_steps: u32, max_steps: u32) -> Self {
        Self {
            id: next_node_id(),
            input: None,
            space_id,
            start_vids: start_vids.to_string(),
            end_vids: None,
            edge_types: Vec::new(),
            direction: EdgeDirection::Both,
            min_steps,
            max_steps,
            edge_alias: None,
            vertex_alias: None,
            e_filter: None,
            e_filter_serializable: None,
            v_filter: None,
            v_filter_serializable: None,
            first_step_filter: None,
            first_step_filter_serializable: None,
            path_semantic: None,
            dst_tag: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn dst_tag(&self) -> Option<&str> {
        self.dst_tag.as_deref()
    }

    pub fn set_dst_tag(&mut self, tag: String) {
        self.dst_tag = Some(tag);
    }

    pub fn set_end_vids(&mut self, end_vids: &str) {
        self.end_vids = Some(end_vids.to_string());
    }

    pub fn set_edge_types(&mut self, edge_types: Vec<String>) {
        self.edge_types = edge_types;
    }

    pub fn set_direction(&mut self, direction: EdgeDirection) {
        self.direction = direction;
    }

    pub fn set_path_semantic(
        &mut self,
        path_semantic: Option<crate::parser::ast::pattern::PathSemantic>,
    ) {
        self.path_semantic = path_semantic;
    }

    pub fn path_semantic(&self) -> Option<crate::parser::ast::pattern::PathSemantic> {
        self.path_semantic.clone()
    }

    pub fn start_vids(&self) -> &str {
        &self.start_vids
    }

    pub fn end_vids(&self) -> Option<&String> {
        self.end_vids.as_ref()
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn direction(&self) -> EdgeDirection {
        self.direction
    }

    pub fn min_steps(&self) -> u32 {
        self.min_steps
    }

    pub fn max_steps(&self) -> u32 {
        self.max_steps
    }

    pub fn step_limit(&self) -> Option<u32> {
        Some(self.max_steps)
    }

    pub fn filter(&self) -> Option<&String> {
        None
    }

    pub fn is_one_step(&self) -> bool {
        self.min_steps == 1 && self.max_steps == 1
    }

    pub fn edge_alias(&self) -> Option<&String> {
        self.edge_alias.as_ref()
    }

    pub fn vertex_alias(&self) -> Option<&String> {
        self.vertex_alias.as_ref()
    }

    pub fn e_filter(&self) -> Option<&ContextualExpression> {
        self.e_filter.as_ref()
    }

    pub fn set_e_filter(&mut self, filter: ContextualExpression) {
        self.e_filter = Some(filter);
        self.e_filter_serializable = None;
    }

    pub fn set_e_filter_expression(
        &mut self,
        filter: Expression,
        ctx: Arc<ExpressionAnalysisContext>,
    ) {
        let expr = linkrs_core::types::expr::ExpressionMeta::new(filter);
        let id = ctx.register_expression(expr);
        self.e_filter = Some(ContextualExpression::new(id, ctx));
        self.e_filter_serializable = None;
    }

    pub fn v_filter(&self) -> Option<&ContextualExpression> {
        self.v_filter.as_ref()
    }

    pub fn set_v_filter(&mut self, filter: ContextualExpression) {
        self.v_filter = Some(filter);
        self.v_filter_serializable = None;
    }

    pub fn set_v_filter_expression(
        &mut self,
        filter: Expression,
        ctx: Arc<ExpressionAnalysisContext>,
    ) {
        let expr = linkrs_core::types::expr::ExpressionMeta::new(filter);
        let id = ctx.register_expression(expr);
        self.v_filter = Some(ContextualExpression::new(id, ctx));
        self.v_filter_serializable = None;
    }

    pub fn first_step_filter(&self) -> Option<&ContextualExpression> {
        self.first_step_filter.as_ref()
    }

    pub fn set_first_step_filter(&mut self, filter: ContextualExpression) {
        self.first_step_filter = Some(filter);
        self.first_step_filter_serializable = None;
    }

    pub fn set_first_step_filter_expression(
        &mut self,
        filter: Expression,
        ctx: Arc<ExpressionAnalysisContext>,
    ) {
        let expr = linkrs_core::types::expr::ExpressionMeta::new(filter);
        let id = ctx.register_expression(expr);
        self.first_step_filter = Some(ContextualExpression::new(id, ctx));
        self.first_step_filter_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) -> Result<(), String> {
        if let Some(ref ctx_expr) = self.e_filter {
            self.e_filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        if let Some(ref ctx_expr) = self.v_filter {
            self.v_filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        if let Some(ref ctx_expr) = self.first_step_filter {
            self.first_step_filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.e_filter_serializable {
            self.e_filter = Some(ser_expr.as_ref().clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.v_filter_serializable {
            self.v_filter = Some(ser_expr.as_ref().clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.first_step_filter_serializable {
            self.first_step_filter = Some(ser_expr.as_ref().clone().to_contextual(ctx));
        }
    }
}
