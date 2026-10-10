use std::sync::Arc;

use crate::define_plan_node;
use crate::planning::plan::core::node_id_generator::next_node_id;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::{ContextualExpression, EdgeDirection, SerializableExpression};

define_plan_node! {
    pub struct ExpandNode {
        space_id: u64,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        step_limit: Option<u32>,
        filter: Option<ContextualExpression>,
        filter_serializable: Option<Box<SerializableExpression>>,
        dst_tag: Option<String>,
    }
    enum: Expand
    input: MultipleInputNode
}

impl ExpandNode {
    pub fn new(space_id: u64, edge_types: Vec<String>, direction: EdgeDirection) -> Self {
        Self {
            id: next_node_id(),
            deps: Vec::new(),
            space_id,
            edge_types,
            direction,
            step_limit: None,
            filter: None,
            filter_serializable: None,
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

    pub fn direction(&self) -> EdgeDirection {
        self.direction
    }

    pub fn set_direction(&mut self, direction: EdgeDirection) {
        self.direction = direction;
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn step_limit(&self) -> Option<u32> {
        self.step_limit
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

    pub fn prepare_for_serialization(&mut self) -> Result<(), String> {
        if let Some(ref ctx_expr) = self.filter {
            self.filter_serializable =
                Some(Box::new(SerializableExpression::from_contextual(ctx_expr)?));
        }
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.filter_serializable {
            self.filter = Some(ser_expr.as_ref().clone().to_contextual(ctx));
        }
    }
}
