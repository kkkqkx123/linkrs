//! Implementation of node traversal in graphs
//!
//! Plan nodes related to graph traversal, such as Expand, ExpandAll, and Traverse.

use std::sync::Arc;

use super::super::super::common::{EdgeProp, TagProp};
use crate::core::types::{ContextualExpression, EdgeDirection, SerializableExpression};
use crate::core::Expression;
use crate::define_binary_input_node;
use crate::define_plan_node;
use crate::define_plan_node_with_deps;
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::validator::context::ExpressionAnalysisContext;

define_plan_node! {
    pub struct ExpandNode {
        space_id: u64,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        step_limit: Option<u32>,
        filter: Option<ContextualExpression>,
        filter_serializable: Option<SerializableExpression>,
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
            output_var: None,
            col_names: Vec::new(),
        }
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
        let expr = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable(filter),
        );
        let id = ctx.register_expression(expr);
        self.filter = Some(ContextualExpression::new(id, ctx));
        self.filter_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        if let Some(ref ctx_expr) = self.filter {
            self.filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.filter_serializable {
            self.filter = Some(ser_expr.clone().to_contextual(ctx));
        }
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for ExpandAllNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "ExpandAllNode"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::Traversal
    }

    fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    fn col_names(&self) -> &[String] {
        &self.col_names
    }

    fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(
            self,
        )
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for ExpandAllNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(
            self.clone(),
        )
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(
            cloned,
        )
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode
    for ExpandAllNode
{
    fn inputs(
        &self,
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    fn inputs_mut(
        &mut self,
    ) -> &mut Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>
    {
        &mut self.deps
    }

    fn add_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.deps.push(input);
    }

    fn remove_input(&mut self, index: usize) -> Result<(), String> {
        if index < self.deps.len() {
            self.deps.remove(index);
            Ok(())
        } else {
            Err(format!("Index {} Out of range", index))
        }
    }
}

/// ExpandAllNode - Plan node for expanding all paths from a starting vertex
///
/// This node is used in MATCH queries to traverse edges and find connected vertices.
/// It can take input from:
/// 1. src_vids - Direct vertex IDs specified in the query
/// 2. input_var - Variable name to look up in ExecutionContext (for joining with previous results)
/// 3. input nodes - Child plan nodes that provide input
#[derive(Debug)]
pub struct ExpandAllNode {
    id: i64,
    deps: Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    space_id: u64,
    edge_types: Vec<String>,
    direction: String,
    any_edge_type: bool,
    step_limit: Option<u32>,
    step_limits: Option<Vec<u32>>,
    join_input: bool,
    sample: bool,
    edge_props: Vec<EdgeProp>,
    vertex_props: Vec<TagProp>,
    filter: Option<ContextualExpression>,
    filter_serializable: Option<SerializableExpression>,
    src_vids: Vec<crate::core::Value>,
    include_empty_paths: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
    /// Input variable name for getting input from ExecutionContext
    input_var: Option<String>,
}

impl Clone for ExpandAllNode {
    fn clone(&self) -> Self {
        use crate::query::planning::plan::core::node_id_generator::next_node_id;
        Self {
            id: next_node_id(),
            deps: self.deps.clone(),
            space_id: self.space_id,
            edge_types: self.edge_types.clone(),
            direction: self.direction.clone(),
            any_edge_type: self.any_edge_type,
            step_limit: self.step_limit,
            step_limits: self.step_limits.clone(),
            join_input: self.join_input,
            sample: self.sample,
            edge_props: self.edge_props.clone(),
            vertex_props: self.vertex_props.clone(),
            filter: self.filter.clone(),
            filter_serializable: self.filter_serializable.clone(),
            src_vids: self.src_vids.clone(),
            include_empty_paths: self.include_empty_paths,
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            input_var: self.input_var.clone(),
        }
    }
}

impl ExpandAllNode {
    pub fn new(space_id: u64, edge_types: Vec<String>, direction: &str) -> Self {
        Self {
            id: next_node_id(),
            deps: Vec::new(),
            space_id,
            edge_types,
            direction: direction.to_string(),
            any_edge_type: false,
            step_limit: None,
            step_limits: None,
            join_input: false,
            sample: false,
            edge_props: Vec::new(),
            vertex_props: Vec::new(),
            filter: None,
            filter_serializable: None,
            src_vids: Vec::new(),
            include_empty_paths: true, // Default to true for backward compatibility
            output_var: None,
            col_names: Vec::new(),
            input_var: None,
        }
    }

    pub fn set_src_vids(&mut self, src_vids: Vec<crate::core::Value>) {
        self.src_vids = src_vids;
    }

    pub fn src_vids(&self) -> &[crate::core::Value] {
        &self.src_vids
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn set_include_empty_paths(&mut self, include: bool) {
        self.include_empty_paths = include;
    }

    pub fn include_empty_paths(&self) -> bool {
        self.include_empty_paths
    }

    pub fn set_any_edge_type(&mut self, any: bool) {
        self.any_edge_type = any;
    }

    pub fn any_edge_type(&self) -> bool {
        self.any_edge_type
    }

    pub fn step_limits(&self) -> Option<&Vec<u32>> {
        self.step_limits.as_ref()
    }

    pub fn set_step_limits(&mut self, limits: Vec<u32>) {
        self.step_limits = Some(limits);
    }

    pub fn join_input(&self) -> bool {
        self.join_input
    }

    pub fn set_join_input(&mut self, join: bool) {
        self.join_input = join;
    }

    pub fn sample(&self) -> bool {
        self.sample
    }

    pub fn set_sample(&mut self, sample: bool) {
        self.sample = sample;
    }

    pub fn edge_props(&self) -> &[EdgeProp] {
        &self.edge_props
    }

    pub fn set_edge_props(&mut self, props: Vec<EdgeProp>) {
        self.edge_props = props;
    }

    pub fn vertex_props(&self) -> &[TagProp] {
        &self.vertex_props
    }

    pub fn set_vertex_props(&mut self, props: Vec<TagProp>) {
        self.vertex_props = props;
    }

    pub fn step_limit(&self) -> Option<u32> {
        self.step_limit
    }

    pub fn set_step_limit(&mut self, limit: u32) {
        self.step_limit = Some(limit);
    }

    pub fn direction(&self) -> &str {
        &self.direction
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
        self.filter_serializable = None;
    }

    pub fn set_filter_string(&mut self, filter: String, ctx: Arc<ExpressionAnalysisContext>) {
        let expr = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable(filter),
        );
        let id = ctx.register_expression(expr);
        self.filter = Some(ContextualExpression::new(id, ctx));
        self.filter_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        if let Some(ref ctx_expr) = self.filter {
            self.filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.filter_serializable {
            self.filter = Some(ser_expr.clone().to_contextual(ctx));
        }
    }

    pub fn get_input_var(&self) -> Option<&str> {
        self.input_var.as_deref()
    }

    pub fn set_input_var(&mut self, input_var: String) {
        self.input_var = Some(input_var);
    }

    pub fn dependencies(
        &self,
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }
}

impl crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable
    for ExpandAllNode
{
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<ExpandAllNode>();

        // Estimate edge_types Vec<String>
        let edge_types_size = std::mem::size_of::<Vec<String>>()
            + self
                .edge_types
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        // Estimate direction String
        let direction_size = std::mem::size_of::<String>() + self.direction.capacity();

        // Estimate step_limits Vec<u32>
        let step_limits_size = self
            .step_limits
            .as_ref()
            .map(|v| std::mem::size_of::<Vec<u32>>() + v.len() * std::mem::size_of::<u32>())
            .unwrap_or(0);

        // Estimate edge_props Vec<EdgeProp>
        let edge_props_size = std::mem::size_of::<Vec<EdgeProp>>()
            + self.edge_props.len() * std::mem::size_of::<EdgeProp>();

        // Estimate vertex_props Vec<TagProp>
        let vertex_props_size = std::mem::size_of::<Vec<TagProp>>()
            + self.vertex_props.len() * std::mem::size_of::<TagProp>();

        // Estimate src_vids Vec<Value>
        let src_vids_size = std::mem::size_of::<Vec<crate::core::Value>>()
            + self.src_vids.len() * std::mem::size_of::<crate::core::Value>();

        // Estimate output_var Option<String>
        let output_var_size = std::mem::size_of::<Option<String>>()
            + self
                .output_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        // Estimate col_names Vec<String>
        let col_names_size = std::mem::size_of::<Vec<String>>()
            + self
                .col_names
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        // Estimate input_var Option<String>
        let input_var_size = std::mem::size_of::<Option<String>>()
            + self
                .input_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        base + edge_types_size
            + direction_size
            + step_limits_size
            + edge_props_size
            + vertex_props_size
            + src_vids_size
            + output_var_size
            + col_names_size
            + input_var_size
    }
}

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
        e_filter_serializable: Option<SerializableExpression>,
        v_filter: Option<ContextualExpression>,
        v_filter_serializable: Option<SerializableExpression>,
        first_step_filter: Option<ContextualExpression>,
        first_step_filter_serializable: Option<SerializableExpression>,
    }
    enum: Traverse
    input: SingleInputNode
}

impl TraverseNode {
    pub fn new(space_id: u64, start_vids: &str, min_steps: u32, max_steps: u32) -> Self {
        Self {
            id: next_node_id(),
            input: None,
            deps: Vec::new(),
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
            output_var: None,
            col_names: Vec::new(),
        }
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
        let expr = crate::core::types::expr::ExpressionMeta::new(filter);
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
        let expr = crate::core::types::expr::ExpressionMeta::new(filter);
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
        let expr = crate::core::types::expr::ExpressionMeta::new(filter);
        let id = ctx.register_expression(expr);
        self.first_step_filter = Some(ContextualExpression::new(id, ctx));
        self.first_step_filter_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        if let Some(ref ctx_expr) = self.e_filter {
            self.e_filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
        if let Some(ref ctx_expr) = self.v_filter {
            self.v_filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
        if let Some(ref ctx_expr) = self.first_step_filter {
            self.first_step_filter_serializable =
                Some(SerializableExpression::from_contextual(ctx_expr));
        }
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.e_filter_serializable {
            self.e_filter = Some(ser_expr.clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.v_filter_serializable {
            self.v_filter = Some(ser_expr.clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.first_step_filter_serializable {
            self.first_step_filter = Some(ser_expr.clone().to_contextual(ctx));
        }
    }
}

define_plan_node! {
    pub struct AppendVerticesNode {
        space_id: u64,
        vertex_tag: String,
        vertex_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        filter_serializable: Option<SerializableExpression>,
        input_var: Option<String>,
        src_expression: Option<ContextualExpression>,
        src_expression_serializable: Option<SerializableExpression>,
        dedup: bool,
        need_fetch_prop: bool,
        vids: Vec<String>,
        tag_ids: Vec<i32>,
        v_filter: Option<ContextualExpression>,
        v_filter_serializable: Option<SerializableExpression>,
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
        let expr = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable(filter),
        );
        let id = ctx.register_expression(expr);
        self.filter = Some(ContextualExpression::new(id, ctx));
        self.filter_serializable = None;
    }

    pub fn input_var(&self) -> Option<&str> {
        self.input_var.as_deref()
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
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
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
        let expr = crate::core::types::expr::ExpressionMeta::new(filter);
        let id = ctx.register_expression(expr);
        self.v_filter = Some(ContextualExpression::new(id, ctx));
        self.v_filter_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        if let Some(ref ctx_expr) = self.filter {
            self.filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
        if let Some(ref ctx_expr) = self.src_expression {
            self.src_expression_serializable =
                Some(SerializableExpression::from_contextual(ctx_expr));
        }
        if let Some(ref ctx_expr) = self.v_filter {
            self.v_filter_serializable = Some(SerializableExpression::from_contextual(ctx_expr));
        }
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.filter_serializable {
            self.filter = Some(ser_expr.clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.src_expression_serializable {
            self.src_expression = Some(ser_expr.clone().to_contextual(ctx.clone()));
        }
        if let Some(ref ser_expr) = self.v_filter_serializable {
            self.v_filter = Some(ser_expr.clone().to_contextual(ctx));
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

define_binary_input_node! {
    pub struct BiExpandNode {
        space_id: u64,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        edge_types: Vec<String>,
        max_hops: usize,
        meeting_point_var: Option<String>,
    }
    enum: BiExpand
    input: BinaryInputNode
}

impl BiExpandNode {
    pub fn new(
        left_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        space_id: u64,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        edge_types: Vec<String>,
        max_hops: usize,
    ) -> Self {
        Self {
            id: next_node_id(),
            left: Box::new(left_input),
            right: Box::new(right_input),
            deps: Vec::new(),
            space_id,
            left_direction,
            right_direction,
            edge_types,
            max_hops,
            meeting_point_var: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn left_direction(&self) -> EdgeDirection {
        self.left_direction
    }

    pub fn right_direction(&self) -> EdgeDirection {
        self.right_direction
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn max_hops(&self) -> usize {
        self.max_hops
    }

    pub fn meeting_point_var(&self) -> Option<&String> {
        self.meeting_point_var.as_ref()
    }

    pub fn set_meeting_point_var(&mut self, var: String) {
        self.meeting_point_var = Some(var);
    }
}

define_binary_input_node! {
    pub struct BiTraverseNode {
        space_id: u64,
        left_src_var: String,
        right_src_var: String,
        edge_types: Vec<String>,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        min_hops: usize,
        max_hops: usize,
        path_var: String,
        edge_alias: Option<String>,
        vertex_alias: Option<String>,
    }
    enum: BiTraverse
    input: BinaryInputNode
}

/// Parameters for creating BiTraverseNode
pub struct BiTraverseNodeParams {
    pub left_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    pub right_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    pub space_id: u64,
    pub left_src_var: String,
    pub right_src_var: String,
    pub edge_types: Vec<String>,
    pub left_direction: EdgeDirection,
    pub right_direction: EdgeDirection,
    pub min_hops: usize,
    pub max_hops: usize,
    pub path_var: String,
}

impl BiTraverseNode {
    pub fn new(params: BiTraverseNodeParams) -> Self {
        Self {
            id: next_node_id(),
            left: Box::new(params.left_input),
            right: Box::new(params.right_input),
            deps: Vec::new(),
            space_id: params.space_id,
            left_src_var: params.left_src_var,
            right_src_var: params.right_src_var,
            edge_types: params.edge_types,
            left_direction: params.left_direction,
            right_direction: params.right_direction,
            min_hops: params.min_hops,
            max_hops: params.max_hops,
            path_var: params.path_var.clone(),
            edge_alias: None,
            vertex_alias: None,
            output_var: Some(params.path_var),
            col_names: vec![],
        }
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn left_src_var(&self) -> &str {
        &self.left_src_var
    }

    pub fn right_src_var(&self) -> &str {
        &self.right_src_var
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn left_direction(&self) -> EdgeDirection {
        self.left_direction
    }

    pub fn right_direction(&self) -> EdgeDirection {
        self.right_direction
    }

    pub fn min_hops(&self) -> usize {
        self.min_hops
    }

    pub fn max_hops(&self) -> usize {
        self.max_hops
    }

    pub fn path_var(&self) -> &str {
        &self.path_var
    }

    pub fn edge_alias(&self) -> Option<&String> {
        self.edge_alias.as_ref()
    }

    pub fn set_edge_alias(&mut self, alias: String) {
        self.edge_alias = Some(alias);
    }

    pub fn vertex_alias(&self) -> Option<&String> {
        self.vertex_alias.as_ref()
    }

    pub fn set_vertex_alias(&mut self, alias: String) {
        self.vertex_alias = Some(alias);
    }
}
