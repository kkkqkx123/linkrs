use std::sync::Arc;

use crate::planning::plan::core::common::{EdgeProp, TagProp};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::{ContextualExpression, SerializableExpression};

impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for ExpandAllNode {
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

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(self)
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for ExpandAllNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(
            self.clone(),
        )
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::ExpandAll(cloned)
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode
    for ExpandAllNode
{
    fn inputs(&self) -> &[crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    fn inputs_mut(
        &mut self,
    ) -> &mut Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum> {
        &mut self.deps
    }

    fn add_input(
        &mut self,
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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
#[derive(Debug, Clone)]
pub struct ExpandAllNode {
    id: i64,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
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
    filter_serializable: Option<Box<SerializableExpression>>,
    src_vids: Vec<linkrs_core::Value>,
    include_empty_paths: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
    /// Input variable name for getting input from ExecutionContext
    input_var: Option<String>,
    /// When true, the destination vertex is emitted as a raw `Value::VertexId`
    /// (and the edge column as `Value::Null`) instead of materializing the full
    /// `Value::Vertex(Box)` / `Value::Edge(Box)`.  Only safe when the
    /// destination is used solely as the seed of a subsequent hop (or not
    /// referenced downstream at all).  Annotated by the optimizer's
    /// `ExpandPushdown` batch.
    id_only: bool,
    /// When true, this is the terminal hop of an expansion chain feeding a
    /// count-only aggregate.  The executor skips output-row materialization
    /// and returns the edge count per chunk.  Implies `id_only`.  Annotated by
    /// the optimizer's `ExpandPushdown` batch.
    count_only: bool,
    /// When true (always alongside `id_only`), the hop's *source* column is
    /// also emitted as a raw `Value::VertexId` instead of cloning the full
    /// `Value::Vertex(Box)` carried in from upstream.  Only safe when the
    /// source variable is not referenced by any ancestor.  Annotated by the
    /// optimizer's `ExpandPushdown` batch.
    lightweight_source: bool,
    /// Path semantic parsed from the pattern (`*TRAIL`, `*ACYCLIC`,
    /// `*SHORTEST`, `*ALL SHORTEST`); `None` means plain walk. Threaded
    /// from `EdgePattern::path_semantic` through the logical node.
    path_semantic: Option<crate::parser::ast::pattern::PathSemantic>,
    dst_tag: Option<String>,
    /// Columnar expand annotation: edge property demand (`None` means the
    /// whole edge value is needed, `Some(vec)` lists the demanded property
    /// names with empty meaning topology only). Annotated by `ExpandPushdown`.
    edge_required_props: Option<Box<Vec<String>>>,
    /// Columnar expand annotation: destination property demand, same encoding
    /// as the edge demand. Empty when the destination is only counted or only
    /// feeds the next hop seed.
    dst_required_props: Option<Box<Vec<String>>>,
    /// Columnar expand annotation: every involved edge type declares endpoint
    /// labels compatible with the plan `dst_tag`, so neighbor labels are fully
    /// determined by schema without per-edge checks.
    closed_loop: bool,
    /// Columnar expand annotation: the direct consumer chain is column-capable
    /// (passthrough/constant project, next seed-tolerant hop, bare count), so
    /// the hop may skip the row view and emit typed columns only.
    skip_rows: bool,
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
            id_only: false,
            count_only: false,
            lightweight_source: false,
            path_semantic: None,
            dst_tag: None,
            edge_required_props: None,
            dst_required_props: None,
            closed_loop: false,
            skip_rows: false,
        }
    }

    pub fn dst_tag(&self) -> Option<&str> {
        self.dst_tag.as_deref()
    }

    pub fn set_dst_tag(&mut self, tag: String) {
        self.dst_tag = Some(tag);
    }

    pub fn set_src_vids(&mut self, src_vids: Vec<linkrs_core::Value>) {
        self.src_vids = src_vids;
    }

    pub fn src_vids(&self) -> &[linkrs_core::Value] {
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

    pub fn get_input_var(&self) -> Option<&str> {
        self.input_var.as_deref()
    }

    pub fn set_input_var(&mut self, input_var: String) {
        self.input_var = Some(input_var);
    }

    pub fn id_only(&self) -> bool {
        self.id_only
    }

    pub fn set_id_only(&mut self, id_only: bool) {
        self.id_only = id_only;
    }

    pub fn count_only(&self) -> bool {
        self.count_only
    }

    pub fn set_count_only(&mut self, count_only: bool) {
        self.count_only = count_only;
    }

    pub fn lightweight_source(&self) -> bool {
        self.lightweight_source
    }

    pub fn set_lightweight_source(&mut self, lightweight_source: bool) {
        self.lightweight_source = lightweight_source;
    }

    pub fn edge_required_props(&self) -> Option<&Vec<String>> {
        self.edge_required_props.as_deref()
    }

    pub fn set_edge_required_props(&mut self, props: Option<Vec<String>>) {
        self.edge_required_props = props.map(Box::new);
    }

    pub fn dst_required_props(&self) -> Option<&Vec<String>> {
        self.dst_required_props.as_deref()
    }

    pub fn set_dst_required_props(&mut self, props: Option<Vec<String>>) {
        self.dst_required_props = props.map(Box::new);
    }

    pub fn closed_loop(&self) -> bool {
        self.closed_loop
    }

    pub fn set_closed_loop(&mut self, closed: bool) {
        self.closed_loop = closed;
    }

    pub fn skip_rows(&self) -> bool {
        self.skip_rows
    }

    pub fn set_skip_rows(&mut self, skip: bool) {
        self.skip_rows = skip;
    }

    pub fn path_semantic(&self) -> Option<crate::parser::ast::pattern::PathSemantic> {
        self.path_semantic.clone()
    }

    pub fn set_path_semantic(
        &mut self,
        path_semantic: Option<crate::parser::ast::pattern::PathSemantic>,
    ) {
        self.path_semantic = path_semantic;
    }

    pub fn dependencies(
        &self,
    ) -> &[crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }
}

impl crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable
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
        let src_vids_size = std::mem::size_of::<Vec<linkrs_core::Value>>()
            + self.src_vids.len() * std::mem::size_of::<linkrs_core::Value>();

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
