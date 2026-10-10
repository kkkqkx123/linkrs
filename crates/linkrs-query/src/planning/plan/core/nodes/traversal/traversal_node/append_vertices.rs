use std::sync::Arc;

use crate::define_plan_node;
use crate::planning::plan::core::common::TagProp;
use crate::planning::plan::core::node_id_generator::next_node_id;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::{ContextualExpression, SerializableExpression};
use linkrs_core::Expression;

define_plan_node! {
    pub struct AppendVerticesNode {
        space_id: u64,
        vertex_tag: String,
        vertex_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        filter_serializable: Option<Box<SerializableExpression>>,
        input_var: Option<String>,
        src_expression: Option<ContextualExpression>,
        src_expression_serializable: Option<Box<SerializableExpression>>,
        dedup: bool,
        need_fetch_prop: bool,
        vids: Vec<String>,
        tag_ids: Vec<i32>,
        v_filter: Option<ContextualExpression>,
        v_filter_serializable: Option<Box<SerializableExpression>>,
        node_alias: Option<String>,
    }
    enum: AppendVertices
    input: MultipleInputNode
}

impl AppendVerticesNode {
    pub fn new(space_id: u64, vertex_tag: &str) -> Self {
        Self {
            id: next_node_id(),
            deps: Vec::new(),
            space_id,
            vertex_tag: vertex_tag.to_string(),
            vertex_props: Vec::new(),
            filter: None,
            filter_serializable: None,
            input_var: None,
            src_expression: None,
            src_expression_serializable: None,
            dedup: false,
            need_fetch_prop: false,
            vids: Vec::new(),
            tag_ids: Vec::new(),
            v_filter: None,
            v_filter_serializable: None,
            node_alias: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn node_alias(&self) -> Option<&String> {
        self.node_alias.as_ref()
    }

    pub fn set_node_alias(&mut self, alias: String) {
        self.node_alias = Some(alias);
    }

    pub fn vertex_tag(&self) -> &str {
        &self.vertex_tag
    }

    pub fn vertex_props(&self) -> &[TagProp] {
        &self.vertex_props
    }

    pub fn set_vertex_props(&mut self, props: Vec<TagProp>) {
        self.vertex_props = props;
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
        self.filter_serializable = None;
    }

    pub fn set_filter_string(&mut self, filter: String, ctx: Arc<ExpressionAnalysisContext>) {
        let expr = linkrs_core::types::expr::ExpressionMeta::new(
            linkrs_core::Expression::Variable(filter),
        );
        let id = ctx.register_expression(expr);
        self.filter = Some(ContextualExpression::new(id, ctx));
        self.filter_serializable = None;
    }

    pub fn input_var(&self) -> Option<&str> {
        self.input_var.as_deref()
    }

    pub fn set_input_var(&mut self, var: String) {
        self.input_var = Some(var);
    }

    pub fn src_expression(&self) -> Option<&ContextualExpression> {
        self.src_expression.as_ref()
    }

    pub fn set_src_expression(&mut self, expr: ContextualExpression) {
        self.src_expression = Some(expr);
        self.src_expression_serializable = None;
    }

    pub fn set_src_expression_expression(
        &mut self,
        expr: Expression,
        ctx: Arc<ExpressionAnalysisContext>,
    ) {
        let meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        self.src_expression = Some(ContextualExpression::new(id, ctx));
        self.src_expression_serializable = None;
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

    pub fn prepare_for_serialization(&mut self) -> Result<(), String> {
        if let Some(ref ctx_expr) = self.filter {
            self.filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        if let Some(ref ctx_expr) = self.src_expression {
            self.src_expression_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        if let Some(ref ctx_expr) = self.v_filter {
            self.v_filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.filter_serializable {
            self.filter = Some(ser_expr.as_ref().clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.src_expression_serializable {
            self.src_expression = Some(ser_expr.as_ref().clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.v_filter_serializable {
            self.v_filter = Some(ser_expr.as_ref().clone().to_contextual(ctx));
        }
    }

    pub fn dedup(&self) -> bool {
        self.dedup
    }

    pub fn need_fetch_prop(&self) -> bool {
        self.need_fetch_prop
    }

    pub fn vids(&self) -> &[String] {
        &self.vids
    }

    pub fn tag_ids(&self) -> &[i32] {
        &self.tag_ids
    }
}
