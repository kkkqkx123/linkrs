//! Implementation of the image scanning node
//!
//! This includes the planning of the steps required to obtain the vertices, edges, and neighboring nodes.
//!
//! # Execution contract of the Get* family (GetVertices/GetEdges/GetNeighbors)
//!
//! `GetVerticesNode` and `GetNeighborsNode` are declared as `MultipleInputNode`
//! and carry a `deps` list, but **the streaming executor never consumes it**:
//! the arena builder lowers every access node to an `InputContract::NoInput`
//! source (`arena_builder/specs.rs` `build_source_spec`), and the source
//! operator resolves the vertex/neighbor sources as follows:
//!
//! - `GetVertices`: vertex IDs are baked into the spec at build time from
//!   `src_ref().constant_value()` or the comma-separated `src_vids` literal
//!   (`specs.rs:119-143`) — a static point lookup, never bound from input rows.
//! - `GetNeighbors`: scans **all** vertices from storage and collects
//!   neighbor IDs per vertex (`operators/source_operator/neighbors.rs`).
//!
//! The `deps` therefore serve the optimizer and EXPLAIN only: merge rules peek
//! at `dependencies()` (e.g. `heuristic/merge/merge_get_vertices_and_project.rs`)
//! and the `FETCH VERTICES` planner links the `ArgumentNode` parameter source
//! through `add_dependency` (`statements/dql/fetch_vertices_planner.rs`). Do not
//! read execution semantics from the `MultipleInputNode` declaration; the
//! declaration is kept as-is so the optimizer rules keep working.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::define_plan_node;
use crate::query::planning::plan::core::common::{EdgeProp, TagProp};
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

define_plan_node! {
    /// Get vertices by point lookup on a static ID list.
    ///
    /// Execution contract: the arena builder resolves `src_vids`/`src_ref` into a
    /// static `vertex_ids` list (`SourceSpec::GetVertices`) and never consumes
    /// `deps`. See the module docs for the full Get* execution contract.
    pub struct GetVerticesNode {
        space_id: u64,
        space_name: String,
        src_ref: ContextualExpression,
        src_vids: String,
        tag_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
        projected_properties: Vec<String>,
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
            filter: None,
            dedup: false,
            limit: None,
            projected_properties: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn projected_properties(&self) -> &[String] {
        &self.projected_properties
    }

    pub fn set_projected_properties(&mut self, properties: Vec<String>) {
        self.projected_properties = properties;
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn has_effective_filter(&self) -> bool {
        self.filter.is_some()
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

    pub fn tag_props(&self) -> &[TagProp] {
        &self.tag_props
    }

    pub fn set_tag_props(&mut self, tag_props: Vec<TagProp>) {
        self.tag_props = tag_props;
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, expression: ContextualExpression) {
        self.filter = Some(expression);
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
    pub struct GetEdgesNode {
        space_id: u64,
        edge_ref: ContextualExpression,
        src: String,
        edge_type: String,
        rank: String,
        dst: String,
        edge_props: Vec<EdgeProp>,
        filter: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
        projected_properties: Vec<String>,
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
            filter: None,
            dedup: false,
            limit: None,
            projected_properties: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn has_effective_filter(&self) -> bool {
        self.filter.is_some()
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

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, expression: ContextualExpression) {
        self.filter = Some(expression);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn edge_props(&self) -> &[EdgeProp] {
        &self.edge_props
    }

    pub fn set_edge_props(&mut self, props: Vec<EdgeProp>) {
        self.edge_props = props;
    }

    pub fn projected_properties(&self) -> &[String] {
        &self.projected_properties
    }

    pub fn set_projected_properties(&mut self, properties: Vec<String>) {
        self.projected_properties = properties;
    }
}

define_plan_node! {
    /// Get neighbor vertices of a source vertex set (OUT/IN/BOTH).
    ///
    /// Execution contract: lowered to `SourceSpec::GetNeighbors`, which at
    /// runtime scans all vertices from storage and collects neighbor IDs per
    /// vertex (`source_operator/neighbors.rs`). `deps` are never consumed at
    /// execution; they serve optimizer/EXPLAIN traversal only.
    pub struct GetNeighborsNode {
        space_id: u64,
        src_vids: String,
        edge_types: Vec<String>,
        direction: String,
        edge_props: Vec<EdgeProp>,
        tag_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
        projected_properties: Vec<String>,
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
            filter: None,
            dedup: false,
            limit: None,
            projected_properties: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn projected_properties(&self) -> &[String] {
        &self.projected_properties
    }

    pub fn set_projected_properties(&mut self, properties: Vec<String>) {
        self.projected_properties = properties;
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

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, expression: ContextualExpression) {
        self.filter = Some(expression);
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
        filter: Option<ContextualExpression>,
        limit: Option<i64>,
        projected_properties: Vec<String>,
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
            filter: None,
            limit: None,
            projected_properties: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
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

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn projected_properties(&self) -> &[String] {
        &self.projected_properties
    }

    pub fn set_projected_properties(&mut self, properties: Vec<String>) {
        self.projected_properties = properties;
    }
}

define_plan_node! {
    pub struct ScanEdgesNode {
        space_id: u64,
        edge_type: Option<String>,
        filter: Option<ContextualExpression>,
        limit: Option<i64>,
        projected_properties: Vec<String>,
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
            filter: None,
            limit: None,
            projected_properties: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
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
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn projected_properties(&self) -> &[String] {
        &self.projected_properties
    }

    pub fn set_projected_properties(&mut self, properties: Vec<String>) {
        self.projected_properties = properties;
    }
}
