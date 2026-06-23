//! Implementation of the image scanning node
//!
//! This includes the planning of the steps required to obtain the vertices, edges, and neighboring nodes.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::define_plan_node;
use crate::query::planning::plan::core::common::{EdgeProp, TagProp};
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::access::index_scan::{IndexLimit, ScanType};
use crate::query::planning::plan::core::nodes::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;

define_plan_node! {
    pub struct GetVerticesNode {
        space_id: u64,
        space_name: String,
        src_ref: ContextualExpression,
        src_vids: String,
        tag_props: Vec<TagProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetVertices
    input: MultipleInputNode
}

impl GetVerticesNode {
    pub fn new(space_id: u64, space_name: &str, src_vids: &str) -> Self {
        use crate::core::types::expr::ExpressionMeta;
        use crate::core::Expression;
        use std::sync::Arc;
        use ExpressionAnalysisContext;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let src_expr = Expression::Variable(src_vids.to_string());
        let src_meta = ExpressionMeta::new(src_expr);
        let src_id = expr_ctx.register_expression(src_meta);
        let src_ctx_expr = ContextualExpression::new(src_id, expr_ctx);

        Self {
            id: next_node_id(),
            deps: Vec::new(),
            space_id,
            space_name: space_name.to_string(),
            src_ref: src_ctx_expr,
            src_vids: src_vids.to_string(),
            tag_props: Vec::new(),
            expression: None,
            dedup: false,
            limit: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn has_effective_filter(&self) -> bool {
        self.expression.is_some()
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn src_vids(&self) -> &str {
        &self.src_vids
    }

    pub fn set_tag_props(&mut self, tag_props: Vec<TagProp>) {
        self.tag_props = tag_props;
    }

    pub fn expression(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_expression(&mut self, expression: ContextualExpression) {
        self.expression = Some(expression);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn dedup(&self) -> bool {
        self.dedup
    }

    pub fn set_dedup(&mut self, dedup: bool) {
        self.dedup = dedup;
    }

    pub fn set_src_vids(&mut self, src_vids: String) {
        use crate::core::types::expr::ExpressionMeta;
        use crate::core::Expression;
        use std::sync::Arc;
        use ExpressionAnalysisContext;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let src_expr = Expression::Variable(src_vids.clone());
        let src_meta = ExpressionMeta::new(src_expr);
        let src_id = expr_ctx.register_expression(src_meta);
        let src_ctx_expr = ContextualExpression::new(src_id, expr_ctx);

        self.src_ref = src_ctx_expr;
        self.src_vids = src_vids;
    }

    pub fn src_ref(&self) -> &ContextualExpression {
        &self.src_ref
    }

    pub fn set_src_ref(&mut self, src_ref: ContextualExpression) {
        self.src_ref = src_ref;
    }

    pub fn deps(&self) -> &[PlanNodeEnum] {
        &self.deps
    }

    pub fn deps_mut(&mut self) -> &mut Vec<PlanNodeEnum> {
        &mut self.deps
    }

    pub fn set_deps(&mut self, deps: Vec<PlanNodeEnum>) {
        self.deps = deps;
    }
}

define_plan_node! {
    pub struct EdgeIndexScanNode {
        space_id: u64,
        edge_type: String,
        index_name: String,
        expression: Option<ContextualExpression>,
        limit: Option<i64>,
        scan_type: ScanType,
        scan_limits: Vec<IndexLimit>,
        return_columns: Vec<String>,
    }
    enum: EdgeIndexScan
    input: ZeroInputNode
}

impl EdgeIndexScanNode {
    pub fn new(space_id: u64, edge_type: &str, index_name: &str) -> Self {
        Self {
            id: next_node_id(),
            space_id,
            edge_type: edge_type.to_string(),
            index_name: index_name.to_string(),
            expression: None,
            limit: None,
            scan_type: ScanType::Full,
            scan_limits: Vec::new(),
            return_columns: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn edge_type(&self) -> &str {
        &self.edge_type
    }

    pub fn schema_name(&self) -> &str {
        &self.edge_type
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.expression = Some(filter);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn scan_type(&self) -> ScanType {
        self.scan_type
    }

    pub fn set_scan_type(&mut self, scan_type: ScanType) {
        self.scan_type = scan_type;
    }

    pub fn scan_limits(&self) -> &[IndexLimit] {
        &self.scan_limits
    }

    pub fn set_scan_limits(&mut self, scan_limits: Vec<IndexLimit>) {
        self.scan_limits = scan_limits;
    }

    pub fn return_columns(&self) -> &[String] {
        &self.return_columns
    }

    pub fn set_return_columns(&mut self, columns: Vec<String>) {
        self.return_columns = columns;
    }
}

define_plan_node! {
    pub struct GetEdgesNode {
        space_id: u64,
        edge_ref: ContextualExpression,
        src: String,
        edge_type: String,
        rank: String,
        dst: String,
        edge_props: Vec<EdgeProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetEdges
    input: ZeroInputNode
}

impl GetEdgesNode {
    pub fn new(space_id: u64, src: &str, edge_type: &str, rank: &str, dst: &str) -> Self {
        use crate::core::types::expr::ExpressionMeta;
        use crate::core::Expression;
        use std::sync::Arc;
        use ExpressionAnalysisContext;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let edge_expr = Expression::Variable(format!("{}->{}@{}", src, dst, edge_type));
        let edge_meta = ExpressionMeta::new(edge_expr);
        let edge_id = expr_ctx.register_expression(edge_meta);
        let edge_ctx_expr = ContextualExpression::new(edge_id, expr_ctx);

        Self {
            id: next_node_id(),
            space_id,
            edge_ref: edge_ctx_expr,
            src: src.to_string(),
            edge_type: edge_type.to_string(),
            rank: rank.to_string(),
            dst: dst.to_string(),
            edge_props: Vec::new(),
            expression: None,
            dedup: false,
            limit: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn has_effective_filter(&self) -> bool {
        self.expression.is_some()
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    pub fn edge_type(&self) -> &str {
        &self.edge_type
    }

    pub fn rank(&self) -> &str {
        &self.rank
    }

    pub fn dst(&self) -> &str {
        &self.dst
    }

    pub fn expression(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_expression(&mut self, expression: ContextualExpression) {
        self.expression = Some(expression);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }
}

define_plan_node! {
    pub struct GetNeighborsNode {
        space_id: u64,
        src_vids: String,
        edge_types: Vec<String>,
        direction: String,
        edge_props: Vec<EdgeProp>,
        tag_props: Vec<TagProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetNeighbors
    input: MultipleInputNode
}

impl GetNeighborsNode {
    pub fn new(space_id: u64, src_vids: &str) -> Self {
        Self {
            id: next_node_id(),
            deps: Vec::new(),
            space_id,
            src_vids: src_vids.to_string(),
            edge_types: Vec::new(),
            direction: "BOTH".to_string(),
            edge_props: Vec::new(),
            tag_props: Vec::new(),
            expression: None,
            dedup: false,
            limit: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_edge_types(&mut self, edge_types: Vec<String>) {
        self.edge_types = edge_types;
    }

    pub fn set_direction(&mut self, direction: &str) {
        self.direction = direction.to_string();
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn src_vids(&self) -> &str {
        &self.src_vids
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn direction(&self) -> &str {
        &self.direction
    }

    pub fn expression(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_expression(&mut self, expression: ContextualExpression) {
        self.expression = Some(expression);
    }

    pub fn edge_props(&self) -> &[EdgeProp] {
        &self.edge_props
    }

    pub fn tag_props(&self) -> &[TagProp] {
        &self.tag_props
    }

    pub fn dedup(&self) -> bool {
        self.dedup
    }

    pub fn set_dedup(&mut self, dedup: bool) {
        self.dedup = dedup;
    }

    pub fn set_src_vids(&mut self, src_vids: String) {
        self.src_vids = src_vids;
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn deps(&self) -> &[PlanNodeEnum] {
        &self.deps
    }

    pub fn deps_mut(&mut self) -> &mut Vec<PlanNodeEnum> {
        &mut self.deps
    }

    pub fn set_deps(&mut self, deps: Vec<PlanNodeEnum>) {
        self.deps = deps;
    }
}

define_plan_node! {
    pub struct ScanVerticesNode {
        space_id: u64,
        space_name: String,
        tag: Option<String>,
        expression: Option<ContextualExpression>,
        limit: Option<i64>,
    }
    enum: ScanVertices
    input: ZeroInputNode
}

impl ScanVerticesNode {
    pub fn new(space_id: u64, space_name: &str) -> Self {
        Self {
            id: next_node_id(),
            space_id,
            space_name: space_name.to_string(),
            tag: None,
            expression: None,
            limit: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_tag(&mut self, tag: &str) {
        self.tag = Some(tag.to_string());
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn tag(&self) -> Option<&String> {
        self.tag.as_ref()
    }

    pub fn tag_filter(&self) -> Option<&String> {
        self.tag.as_ref()
    }

    pub fn vertex_filter(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_vertex_filter(&mut self, filter: ContextualExpression) {
        self.expression = Some(filter);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }
}

define_plan_node! {
    pub struct ScanEdgesNode {
        space_id: u64,
        edge_type: Option<String>,
        expression: Option<ContextualExpression>,
        limit: Option<i64>,
    }
    enum: ScanEdges
    input: ZeroInputNode
}

impl ScanEdgesNode {
    pub fn new(space_id: u64, edge_type: &str) -> Self {
        Self {
            id: next_node_id(),
            space_id,
            edge_type: Some(edge_type.to_string()),
            expression: None,
            limit: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn edge_type(&self) -> Option<String> {
        self.edge_type.clone()
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.expression.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.expression = Some(filter);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }
}
