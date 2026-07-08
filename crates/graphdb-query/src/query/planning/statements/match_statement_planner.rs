//! Unified MATCH Statement Planner
//!
//! Implement the StatementPlanner interface to handle the complete planning of MATCH queries.
//! It integrates the following functions:
//!   - Node and edge pattern matching (supports multiple paths)
//!   - WHERE condition filtering
//!   - RETURN Projection
//!   - ORDER BY: Sorting
//!   - LIMIT/SKIP – Pagination options
//!   - Selection of intelligent scanning strategies (index scanning, attribute scanning, full table scanning)

use crate::core::types::ContextualExpression;
use crate::query::metadata::{IndexMetadata, MetadataContext};
use crate::query::parser::ast::pattern::{PathElement, Pattern, RepetitionType};
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::access::index_scan::{
    IndexLimit, IndexScanNode, ScanType,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::core::nodes::ExpandAllNode;
use crate::query::planning::plan::core::nodes::{
    ArgumentNode, LeftJoinNode, LoopNode, ScanVerticesNode, UnionNode,
};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::planning::statements::clauses::{
    OrderByClausePlanner, PaginationPlanner, ReturnClausePlanner, WhereClausePlanner,
};
use crate::query::planning::statements::statement_planner::{ClausePlanner, StatementPlanner};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::validator::ValidationInfo;
use crate::query::QueryContext;
use std::sync::Arc;

/// Pagination Information Structure
#[derive(Debug, Clone)]
pub struct PaginationInfo {
    pub skip: usize,
    pub limit: usize,
}

/// MATCH Statement Planner
///
/// Responsible for converting MATCH queries into executable execution plans.
/// Implement the StatementPlanner interface to provide a unified planning entry point.
/// Delegates clause-level planning (WHERE, RETURN, ORDER BY, LIMIT) to ClausePlanner implementations.
#[derive(Debug, Clone)]
pub struct MatchStatementPlanner {
    config: MatchPlannerConfig,
    expr_context: Option<Arc<ExpressionAnalysisContext>>,
    metadata_context: Option<MetadataContext>,
    where_planner: WhereClausePlanner,
    return_planner: ReturnClausePlanner,
    order_by_planner: OrderByClausePlanner,
    pagination_planner: PaginationPlanner,
}

#[derive(Debug, Clone)]
pub struct MatchPlannerConfig {
    pub default_limit: usize,
    pub max_limit: usize,
    pub enable_index_optimization: bool,
}

impl Default for MatchPlannerConfig {
    fn default() -> Self {
        Self {
            default_limit: 10000,
            max_limit: 100000,
            enable_index_optimization: true,
        }
    }
}

impl Default for MatchStatementPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchStatementPlanner {
    pub fn new() -> Self {
        Self {
            config: MatchPlannerConfig::default(),
            expr_context: None,
            metadata_context: None,
            where_planner: WhereClausePlanner::new(),
            return_planner: ReturnClausePlanner::new(),
            order_by_planner: OrderByClausePlanner::new(),
            pagination_planner: PaginationPlanner::new(),
        }
    }

    pub fn with_config(config: MatchPlannerConfig) -> Self {
        Self {
            config,
            expr_context: None,
            metadata_context: None,
            where_planner: WhereClausePlanner::new(),
            return_planner: ReturnClausePlanner::new(),
            order_by_planner: OrderByClausePlanner::new(),
            pagination_planner: PaginationPlanner::new(),
        }
    }
}

impl Planner for MatchStatementPlanner {
    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Match(_))
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Set expr_context
        self.expr_context = Some(validated.ast.expr_context().clone());

        // Check the optimization suggestions.
        for hint in &validation_info.optimization_hints {
            log::debug!("Optimization Tip: {:?}", hint);
        }

        // Optimize the planning using alias mapping.
        self.plan_match_pattern(validated, space_id, &space_name, validation_info, &qctx)
    }

    fn transform_with_metadata(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        // Store metadata context for use during planning
        self.metadata_context = Some(metadata_context.clone());

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Set expr_context
        self.expr_context = Some(validated.ast.expr_context().clone());

        // Check the optimization suggestions.
        for hint in &validation_info.optimization_hints {
            log::debug!("Optimization Tip: {:?}", hint);
        }

        // Log available indexes for debugging
        if self.config.enable_index_optimization {
            for index in metadata_context.get_all_indexes() {
                log::debug!(
                    "Available index: {} on tag {} field {}",
                    index.index_name,
                    index.tag_name,
                    index.field_name
                );
            }
        }

        // Optimize the planning using alias mapping and metadata.
        self.plan_match_pattern(validated, space_id, &space_name, validation_info, &qctx)
    }
}

impl StatementPlanner for MatchStatementPlanner {
    fn statement_type(&self) -> &'static str {
        "MATCH"
    }

    fn supported_clause_kinds(&self) -> &[CypherClauseKind] {
        const SUPPORTED_CLAUSES: &[CypherClauseKind] = &[
            CypherClauseKind::Match,
            CypherClauseKind::Where,
            CypherClauseKind::Return,
            CypherClauseKind::OrderBy,
            CypherClauseKind::Pagination,
        ];
        SUPPORTED_CLAUSES
    }
}

impl MatchStatementPlanner {
    fn plan_match_pattern(
        &mut self,
        validated: &ValidatedStatement,
        space_id: u64,
        space_name: &str,
        validation_info: &ValidationInfo,
        qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        match stmt {
            crate::query::parser::ast::Stmt::Match(match_stmt) => {
                for hint in &validation_info.index_hints {
                    if hint.estimated_selectivity < 0.1 {
                        log::debug!("Using highly selective indexing: {}", hint.index_name);
                    }
                }

                let referenced_tags = &validation_info.semantic_info.referenced_tags;
                if !referenced_tags.is_empty() {
                    log::debug!("Quoted tags: {:?}", referenced_tags);
                }

                let mut plan = if match_stmt.patterns.is_empty() {
                    self.plan_node_pattern(space_id, space_name)?
                } else {
                    let first_pattern = &match_stmt.patterns[0];
                    self.plan_path_pattern(
                        first_pattern,
                        space_id,
                        space_name,
                        validation_info,
                        qctx,
                    )?
                };

                for pattern in match_stmt.patterns.iter().skip(1) {
                    let path_plan = self.plan_path_pattern(
                        pattern,
                        space_id,
                        space_name,
                        validation_info,
                        qctx,
                    )?;
                    plan = self.cross_join_plans(plan, path_plan)?;
                }

                if self.has_where_clause(stmt) {
                    plan = self
                        .where_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if self.has_return_clause(stmt) {
                    let distinct = extract_distinct_flag_from_stmt(stmt);
                    self.return_planner.set_distinct(distinct);
                    plan = self
                        .return_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if self.has_order_by_clause(stmt) {
                    plan = self
                        .order_by_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if self.has_pagination(stmt) {
                    plan = self
                        .pagination_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if let Some(delete_clause) = &match_stmt.delete_clause {
                    plan = self.plan_match_delete(plan, delete_clause, space_name, match_stmt)?;
                }

                Ok(plan)
            }
            _ => Err(PlannerError::InvalidOperation(
                "Expected MATCH statement".to_string(),
            )),
        }
    }

    /// Planning Path Mode
    fn plan_path_pattern(
        &self,
        pattern: &Pattern,
        space_id: u64,
        space_name: &str,
        validation_info: &ValidationInfo,
        qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        match pattern {
            Pattern::Path(path) => {
                if path.elements.is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "empty path model".to_string(),
                    ));
                }

                let mut plan = SubPlan::new(None, None);
                let mut prev_node_alias: Option<String> = None;
                let mut is_first_node = true;
                let mut is_first_edge = true;

                // Convert elements to a vector for indexed access
                let elements: Vec<_> = path.elements.iter().collect();
                let mut i = 0;

                while i < elements.len() {
                    match elements[i] {
                        PathElement::Node(node) => {
                            if is_first_node {
                                // First node: scan all vertices
                                let node_plan =
                                    self.plan_pattern_node(node, space_id, space_name)?;
                                plan = if let Some(existing_root) = plan.root.take() {
                                    self.cross_join_plans(
                                        SubPlan::new(Some(existing_root), plan.tail),
                                        node_plan,
                                    )?
                                } else {
                                    node_plan
                                };
                                // Use synthetic name for anonymous nodes
                                let node_alias =
                                    node.variable.clone().unwrap_or_else(|| "n".to_string());
                                prev_node_alias = Some(node_alias);
                                is_first_node = false;
                            } else {
                                // Subsequent nodes: use dst column from previous edge expansion
                                // No need to scan vertices - just update the variable alias
                                // The actual node data comes from the edge expansion's dst column
                                // Use synthetic name for anonymous nodes
                                let node_alias =
                                    node.variable.clone().unwrap_or_else(|| "n".to_string());
                                prev_node_alias = Some(node_alias);
                            }
                            i += 1;
                        }
                        PathElement::Edge(edge) => {
                            if prev_node_alias.is_none() {
                                return Err(PlannerError::PlanGenerationFailed(
                                    "The edge pattern must follow the node pattern".to_string(),
                                ));
                            }

                            let input_alias = prev_node_alias.as_deref().unwrap();

                            // Look ahead to find the next node's variable name
                            // This will be used as the dst column name in ExpandAll
                            let dst_var = if i + 1 < elements.len() {
                                if let PathElement::Node(next_node) = elements[i + 1] {
                                    next_node.variable.as_deref()
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            // Create edge expansion plan with proper input variable and dst variable
                            let edge_plan = self.plan_pattern_edge_with_input(
                                edge,
                                space_id,
                                input_alias,
                                dst_var,
                            )?;

                            plan = if let Some(existing_root) = plan.root.take() {
                                if is_first_edge {
                                    // For the first edge, directly connect the node scan to edge expansion
                                    // by setting the input_var of ExpandAllNode to use the output of ScanVerticesNode
                                    // This avoids the Cartesian product issue
                                    self.connect_node_to_edge_expansion(
                                        SubPlan::new(Some(existing_root), plan.tail),
                                        edge_plan,
                                        input_alias,
                                    )?
                                } else {
                                    // For subsequent edges, use HashInnerJoin to connect
                                    // The previous edge's dst column should match with the next edge's src column
                                    let prev_dst_var = input_alias;
                                    self.join_edge_expansions(
                                        SubPlan::new(Some(existing_root), plan.tail),
                                        edge_plan,
                                        prev_dst_var,
                                        self.expr_context.as_ref().ok_or_else(|| {
                                            PlannerError::PlanGenerationFailed(
                                                "Expression context is unavailable".to_string(),
                                            )
                                        })?,
                                    )?
                                }
                            } else {
                                edge_plan
                            };

                            is_first_edge = false;

                            // After edge expansion, the next node should use the dst column
                            // which is named after the next node's variable
                            prev_node_alias = dst_var.map(|s| s.to_string());
                            i += 1;
                        }
                        PathElement::Alternative(patterns) => {
                            let alt_plan = self.plan_alternative_patterns(
                                patterns,
                                space_id,
                                space_name,
                                prev_node_alias.as_deref(),
                                validation_info,
                                qctx,
                            )?;
                            plan = if let Some(existing_root) = plan.root.take() {
                                self.cross_join_plans(
                                    SubPlan::new(Some(existing_root), plan.tail),
                                    alt_plan,
                                )?
                            } else {
                                alt_plan
                            };
                        }
                        PathElement::Optional(elem) => {
                            let opt_plan = self.plan_optional_element(
                                elem,
                                space_id,
                                space_name,
                                prev_node_alias.as_deref(),
                                validation_info,
                                qctx,
                            )?;
                            plan = if let Some(existing_root) = plan.root.take() {
                                self.left_join_plans(
                                    SubPlan::new(Some(existing_root), plan.tail),
                                    opt_plan,
                                )?
                            } else {
                                opt_plan
                            };
                        }
                        PathElement::Repeated(elem, rep_type) => {
                            let rep_plan = self.plan_repeated_element(
                                elem,
                                *rep_type,
                                space_id,
                                space_name,
                                self.expr_context.as_ref().ok_or_else(|| {
                                    PlannerError::PlanGenerationFailed(
                                        "Expression context is unavailable".to_string(),
                                    )
                                })?,
                            )?;
                            plan = if let Some(existing_root) = plan.root.take() {
                                self.cross_join_plans(
                                    SubPlan::new(Some(existing_root), plan.tail),
                                    rep_plan,
                                )?
                            } else {
                                rep_plan
                            };
                        }
                    }
                }

                Ok(plan)
            }
            _ => self.plan_pattern(pattern, space_id, space_name, validation_info, qctx),
        }
    }

    /// Planning Mode Node
    fn plan_pattern_node(
        &self,
        node: &crate::query::parser::ast::pattern::NodePattern,
        space_id: u64,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let var_name = node.variable.clone().unwrap_or_else(|| "n".to_string());

        // Try to use index scan if available
        if self.config.enable_index_optimization {
            if let Some(index_plan) =
                self.try_create_index_scan_plan(node, space_id, space_name, &var_name)?
            {
                return Ok(index_plan);
            }
        }

        // Fall back to full table scan
        let mut scan_node = ScanVerticesNode::new(space_id, space_name);
        scan_node.set_col_names(vec![var_name.clone()]);
        scan_node.set_output_var(var_name.clone());
        let mut plan = SubPlan::from_root(scan_node.into_enum());

        // If there is a label filtering option, please add the filter.
        if !node.labels.is_empty() {
            let expr_ctx = self
                .expr_context
                .as_ref()
                .expect("expr_context should be set");
            let label_filter =
                Self::build_label_filter_expression(&node.variable, &node.labels, expr_ctx);
            let root_node = plan.root.as_ref().expect("The root of plan should exist");
            let filter_node = FilterNode::new(root_node.clone(), label_filter)
                .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }

        // If there is attribute filtering, add the filter.
        if let Some(ref props) = node.properties {
            // Convert property map to filter expression
            let filter_expr = if let Some(ref expr_ctx) = self.expr_context {
                Self::convert_properties_to_filter(&var_name, props, expr_ctx)
            } else {
                None
            };

            let filter_expr = match filter_expr {
                Some(expr) => expr,
                None => props.clone(),
            };

            let filter_node = FilterNode::new(
                plan.root
                    .as_ref()
                    .expect("The root of plan should exist")
                    .clone(),
                filter_expr,
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }

        // If there is predicate filtering, add the filter.
        if !node.predicates.is_empty() {
            for pred in &node.predicates {
                let filter_node = FilterNode::new(
                    plan.root
                        .as_ref()
                        .expect("The root of plan should exist")
                        .clone(),
                    pred.clone(),
                )
                .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
                plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
            }
        }

        Ok(plan)
    }

    /// Try to create an index scan plan for the given node pattern
    fn try_create_index_scan_plan(
        &self,
        node: &crate::query::parser::ast::pattern::NodePattern,
        space_id: u64,
        _space_name: &str,
        var_name: &str,
    ) -> Result<Option<SubPlan>, PlannerError> {
        // Check if we have metadata context
        let metadata_ctx = match &self.metadata_context {
            Some(ctx) => ctx,
            None => return Ok(None),
        };

        // Check if node has labels (tags)
        if node.labels.is_empty() {
            return Ok(None);
        }

        // Get the first label (tag name)
        let tag_name = &node.labels[0];

        // Find a suitable index for this tag
        let suitable_index = self.find_suitable_index(metadata_ctx, tag_name, node, var_name)?;

        match suitable_index {
            Some((index, index_limits)) => {
                // Create IndexScanNode
                let mut index_scan_node = IndexScanNode::new(
                    space_id,
                    0, // tag_id will be resolved later
                    0, // index_id will be resolved later
                    index.index_name.clone(),
                    tag_name.clone(),
                    if index_limits.len() == 1 && index_limits[0].scan_type == ScanType::Unique {
                        ScanType::Unique
                    } else {
                        ScanType::Range
                    },
                );

                index_scan_node.set_scan_limits(index_limits);
                index_scan_node.set_col_names(vec![var_name.to_string()]);
                index_scan_node.set_output_var(var_name.to_string());

                let plan = SubPlan::from_root(index_scan_node.into_enum());
                log::debug!(
                    "Created IndexScanNode for tag '{}' using index '{}'",
                    tag_name,
                    index.index_name
                );
                Ok(Some(plan))
            }
            None => Ok(None),
        }
    }

    /// Find a suitable index for the given node pattern
    fn find_suitable_index(
        &self,
        metadata_ctx: &MetadataContext,
        tag_name: &str,
        node: &crate::query::parser::ast::pattern::NodePattern,
        var_name: &str,
    ) -> Result<Option<(IndexMetadata, Vec<IndexLimit>)>, PlannerError> {
        // Get tag metadata
        let tag_metadata = match metadata_ctx.get_tag_metadata(tag_name) {
            Some(meta) => meta,
            None => return Ok(None),
        };

        // Check if tag has any indexes
        if tag_metadata.indexes.is_empty() {
            return Ok(None);
        }

        // Extract filter conditions from node properties and predicates
        let filter_conditions = self.extract_filter_conditions(node, var_name);

        if filter_conditions.is_empty() {
            return Ok(None);
        }

        // Find an index that matches one of the filter conditions
        for index_name in &tag_metadata.indexes {
            if let Some(index_meta) = metadata_ctx.get_index_metadata(index_name) {
                // Check if any filter condition matches the indexed field
                for (field, op, value) in &filter_conditions {
                    if &index_meta.field_name == field {
                        let index_limit = match op.as_str() {
                            "=" => Some(IndexLimit::equal(field.clone(), value.clone())),
                            ">" => Some(IndexLimit::range(
                                field.clone(),
                                Some(value.clone()) as Option<String>,
                                None::<String>,
                                false,
                                false,
                            )),
                            "<" => Some(IndexLimit::range(
                                field.clone(),
                                None::<String>,
                                Some(value.clone()) as Option<String>,
                                false,
                                false,
                            )),
                            ">=" => Some(IndexLimit::range(
                                field.clone(),
                                Some(value.clone()) as Option<String>,
                                None::<String>,
                                true,
                                false,
                            )),
                            "<=" => Some(IndexLimit::range(
                                field.clone(),
                                None::<String>,
                                Some(value.clone()) as Option<String>,
                                false,
                                true,
                            )),
                            _ => None,
                        };

                        if let Some(limit) = index_limit {
                            return Ok(Some((index_meta.clone(), vec![limit])));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Extract filter conditions from node properties and predicates
    fn extract_filter_conditions(
        &self,
        node: &crate::query::parser::ast::pattern::NodePattern,
        var_name: &str,
    ) -> Vec<(String, String, String)> {
        let mut conditions = Vec::new();

        // Extract from node properties (e.g., {name: "value"})
        if let Some(ref props) = node.properties {
            if let Some(expr_meta) = props.expression() {
                Self::extract_conditions_from_expression(
                    expr_meta.inner(),
                    var_name,
                    &mut conditions,
                );
            }
        }

        // Extract from predicates
        for pred in &node.predicates {
            if let Some(expr_meta) = pred.expression() {
                Self::extract_conditions_from_expression(
                    expr_meta.inner(),
                    var_name,
                    &mut conditions,
                );
            }
        }

        conditions
    }

    /// Extract conditions from an expression
    fn extract_conditions_from_expression(
        expr: &crate::core::types::expr::Expression,
        var_name: &str,
        conditions: &mut Vec<(String, String, String)>,
    ) {
        use crate::core::types::expr::Expression;
        use crate::core::types::operators::BinaryOperator;

        match expr {
            Expression::Binary { left, op, right } => {
                // Handle AND operator by recursively extracting conditions
                if matches!(op, BinaryOperator::And) {
                    Self::extract_conditions_from_expression(left, var_name, conditions);
                    Self::extract_conditions_from_expression(right, var_name, conditions);
                    return;
                }

                let op_str = op.to_string();

                // Check for pattern: var.property op value
                if let Expression::Property { object, property } = left.as_ref() {
                    if let Expression::Variable(obj_name) = object.as_ref() {
                        if obj_name == var_name {
                            if let Expression::Literal(lit) = right.as_ref() {
                                if let Some(value_str) = Self::value_to_index_string(lit) {
                                    conditions.push((property.clone(), op_str.clone(), value_str));
                                }
                            }
                        }
                    }
                }

                // Check for pattern: value op var.property (reversed)
                if let Expression::Property { object, property } = right.as_ref() {
                    if let Expression::Variable(obj_name) = object.as_ref() {
                        if obj_name == var_name {
                            if let Expression::Literal(lit) = left.as_ref() {
                                let reversed_op = match op {
                                    BinaryOperator::GreaterThan => "<".to_string(),
                                    BinaryOperator::LessThan => ">".to_string(),
                                    BinaryOperator::GreaterThanOrEqual => "<=".to_string(),
                                    BinaryOperator::LessThanOrEqual => ">=".to_string(),
                                    _ => op_str.clone(),
                                };
                                if let Some(value_str) = Self::value_to_index_string(lit) {
                                    conditions.push((property.clone(), reversed_op, value_str));
                                }
                            }
                        }
                    }
                }
            }
            Expression::Map(pairs) => {
                for (key, value_expr) in pairs {
                    if let Expression::Literal(lit) = value_expr {
                        if let Some(value_str) = Self::value_to_index_string(lit) {
                            conditions.push((key.clone(), "=".to_string(), value_str));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn value_to_index_string(value: &crate::core::Value) -> Option<String> {
        use crate::core::Value;
        match value {
            Value::String(s) => Some(s.clone()),
            Value::SmallInt(i) => Some(i.to_string()),
            Value::Int(i) => Some(i.to_string()),
            Value::BigInt(i) => Some(i.to_string()),
            Value::Float(f) => Some(f.to_string()),
            Value::Double(d) => Some(d.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Planning mode sidebar
    fn plan_pattern_edge(
        &self,
        edge: &crate::query::parser::ast::pattern::EdgePattern,
        space_id: u64,
        _space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let direction = match edge.direction {
            crate::query::parser::ast::types::EdgeDirection::Out => "out",
            crate::query::parser::ast::types::EdgeDirection::In => "in",
            crate::query::parser::ast::types::EdgeDirection::Both => "both",
        };

        let edge_types = match &edge.edge_types {
            types if !types.is_empty() => types.clone(),
            _ => vec![],
        };

        let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

        if edge.edge_types.is_empty() {
            expand_node.set_any_edge_type(true);
        }

        // Set step limit to 1 for single edge pattern like (n)-[e]->(m)
        expand_node.set_step_limit(1);

        // Set the column name to the edge variable name so that subsequent join operations can find the variable
        let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
        expand_node.set_col_names(vec![edge_var.clone()]);

        let mut plan = SubPlan::from_root(expand_node.into_enum());

        // If there is attribute filtering, add the filter.
        if let Some(ref props) = edge.properties {
            // Convert property map to filter expression
            let filter_expr = if let Some(ref expr_ctx) = self.expr_context {
                Self::convert_properties_to_filter(&edge_var, props, expr_ctx)
            } else {
                None
            };

            let filter_expr = match filter_expr {
                Some(expr) => expr,
                None => props.clone(),
            };

            let filter_node = FilterNode::new(
                plan.root
                    .as_ref()
                    .expect("The root of plan should exist")
                    .clone(),
                filter_expr,
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }

        // If there is predicate filtering, add the filter.
        if !edge.predicates.is_empty() {
            for pred in &edge.predicates {
                let filter_node = FilterNode::new(
                    plan.root
                        .as_ref()
                        .expect("The root of plan should exist")
                        .clone(),
                    pred.clone(),
                )
                .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
                plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
            }
        }

        // For edge-only patterns (no source node), the ExpandAllNode will use its fallback
        // to scan all vertices from the space. This is handled in ExpandAllExecutor::do_execute
        // when input_nodes is empty and input_executor is None.
        Ok(plan)
    }

    /// Planning mode edge with input variable
    ///
    /// This method creates an edge expansion plan that takes the source node as input.
    /// The ExpandAll node will use the input variable to get the source vertices from ExecutionContext.
    ///
    /// # Arguments
    /// * `edge` - The edge pattern to plan
    /// * `space_id` - The space ID
    /// * `input_var` - The variable name for the source node
    /// * `dst_var` - Optional variable name for the destination node. If provided, the dst column will use this name.
    fn plan_pattern_edge_with_input(
        &self,
        edge: &crate::query::parser::ast::pattern::EdgePattern,
        space_id: u64,
        input_var: &str,
        dst_var: Option<&str>,
    ) -> Result<SubPlan, PlannerError> {
        let direction = match edge.direction {
            crate::query::parser::ast::types::EdgeDirection::Out => "out",
            crate::query::parser::ast::types::EdgeDirection::In => "in",
            crate::query::parser::ast::types::EdgeDirection::Both => "both",
        };

        let edge_types = match &edge.edge_types {
            types if !types.is_empty() => types.clone(),
            _ => vec![],
        };

        let mut expand_node = ExpandAllNode::new(space_id, edge_types, direction);

        if edge.edge_types.is_empty() {
            expand_node.set_any_edge_type(true);
        }

        // Set step limit to 1 for single edge pattern like (n)-[e]->(m)
        expand_node.set_step_limit(1);

        // Set the input variable so ExpandAll can get source vertices from ExecutionContext
        expand_node.set_input_var(input_var.to_string());

        // Set the column names to match ExpandAll's output format: [input_var, edge_var, dst_var]
        // Use input_var as the first column name so subsequent operations can reference the source node
        // Use edge variable name if provided, otherwise use "edge" for the edge column
        // Use dst_var if provided, otherwise use "dst" for the destination column
        // This allows subsequent operations to reference both source and destination nodes by their variable names
        let src_col_name = input_var.to_string();
        let edge_col_name = edge.variable.clone().unwrap_or_else(|| "edge".to_string());
        let dst_col_name = dst_var.unwrap_or("dst").to_string();
        expand_node.set_col_names(vec![src_col_name, edge_col_name, dst_col_name]);

        // Disable empty paths for MATCH queries - we only want actual edge expansions
        expand_node.set_include_empty_paths(false);

        let mut plan = SubPlan::from_root(expand_node.into_enum());

        // If there is attribute filtering, add the filter.
        if let Some(ref props) = edge.properties {
            let edge_var = edge.variable.clone().unwrap_or_else(|| "e".to_string());
            // Convert property map to filter expression
            let filter_expr = if let Some(ref expr_ctx) = self.expr_context {
                Self::convert_properties_to_filter(&edge_var, props, expr_ctx)
            } else {
                None
            };

            let filter_expr = match filter_expr {
                Some(expr) => expr,
                None => props.clone(),
            };

            let filter_node = FilterNode::new(
                plan.root
                    .as_ref()
                    .expect("The root of plan should exist")
                    .clone(),
                filter_expr,
            )
            .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
            plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
        }

        // If there is predicate filtering, add the filter.
        if !edge.predicates.is_empty() {
            for pred in &edge.predicates {
                let filter_node = FilterNode::new(
                    plan.root
                        .as_ref()
                        .expect("The root of plan should exist")
                        .clone(),
                    pred.clone(),
                )
                .map_err(|e| PlannerError::PlanGenerationFailed(e.to_string()))?;
                plan = SubPlan::new(Some(filter_node.into_enum()), plan.tail);
            }
        }

        Ok(plan)
    }

    /// Interconnecting two plans
    fn cross_join_plans(&self, left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
        use crate::query::planning::plan::core::nodes::CrossJoinNode;

        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        let (right_root, needs_id_update) = if let Some(expand_all) = right_root.as_expand_all() {
            if expand_all.get_input_var().is_none() {
                // Use a marker that indicates we need to update with the actual ID later
                let marker_var = "__CROSSJOIN_ID_MARKER__".to_string();
                let mut new_expand = expand_all.clone();
                new_expand.set_input_var(marker_var);
                (new_expand.into_enum(), true)
            } else {
                (right_root.clone(), false)
            }
        } else {
            (right_root.clone(), false)
        };

        // Create the join node
        let mut join_node = CrossJoinNode::new(left_root.clone(), right_root.clone())
            .map_err(|e| PlannerError::JoinFailed(format!("Cross-connection failed: {}", e)))?;

        // If we used a marker, update it with the actual join node ID
        if needs_id_update {
            let join_id = join_node.id();
            let actual_var = format!("left_{}", join_id);

            // Update the right child (ExpandAllNode) with the actual variable name
            if let Some(expand_all) = join_node.right_input().as_expand_all() {
                let mut new_expand = expand_all.clone();
                new_expand.set_input_var(actual_var);
                // Recreate the join node with the updated right child
                join_node =
                    CrossJoinNode::new(left_root.clone(), new_expand.into_enum()).map_err(|e| {
                        PlannerError::JoinFailed(format!("Cross-connection failed: {}", e))
                    })?;
            }
        }

        // Set the output_var of the CrossJoinNode to match the left_var
        // This ensures that parent nodes (like HashInnerJoin) can find the result
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
        let output_var = if let Some(expand_all) = join_node.right_input().as_expand_all() {
            expand_all.get_input_var().map(|v| v.to_string())
        } else {
            None
        };

        if let Some(var) = output_var {
            join_node.set_output_var(var);
        }

        Ok(SubPlan {
            root: Some(join_node.into_enum()),
            tail: left.tail.or(right.tail),
        })
    }

    /// Connect a node scan plan to an edge expansion plan
    ///
    /// This method directly connects the node scan output to the edge expansion input
    /// by adding the node scan as an input dependency of the ExpandAllNode.
    /// This avoids the Cartesian product issue that occurs with CrossJoinNode.
    fn connect_node_to_edge_expansion(
        &self,
        node_plan: SubPlan,
        edge_plan: SubPlan,
        node_alias: &str,
    ) -> Result<SubPlan, PlannerError> {
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
            MultipleInputNode, PlanNode,
        };

        let node_root = node_plan.root.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Node plan has no root".to_string())
        })?;

        let edge_root = edge_plan.root.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Edge plan has no root".to_string())
        })?;

        // If the edge root is an ExpandAllNode, add the node scan as an input
        if let Some(expand_all) = edge_root.as_expand_all() {
            let mut new_expand = expand_all.clone();

            // Add the node scan as an input dependency
            // This ensures that the ExpandAllNode will receive the node scan's output as input
            new_expand.add_input(node_root.clone());

            // Set the input_var to the node_alias so ExpandAllExecutor can find the input
            // The node scan's output will be stored in ExecutionContext under this variable name
            new_expand.set_input_var(node_alias.to_string());

            // Set the output_var to help subsequent operations find the result
            new_expand.set_output_var(format!("expand_{}", new_expand.id()));

            Ok(SubPlan {
                root: Some(new_expand.into_enum()),
                tail: node_plan.tail.or(edge_plan.tail),
            })
        } else {
            // If not an ExpandAllNode, fall back to cross_join_plans
            self.cross_join_plans(node_plan, edge_plan)
        }
    }

    /// Join two edge expansion plans
    ///
    /// This method connects the output of a previous edge expansion (dst column)
    /// to the input of the next edge expansion (src column).
    /// The connection is made by adding the left plan as an input dependency of the right plan's ExpandAllNode.
    fn join_edge_expansions(
        &self,
        left_plan: SubPlan,
        right_plan: SubPlan,
        left_dst_alias: &str,
        _expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<SubPlan, PlannerError> {
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;

        let left_root = left_plan.root.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root".to_string())
        })?;

        let right_root = right_plan.root.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root".to_string())
        })?;

        // If right_root is an ExpandAllNode, add the left plan as an input
        if let Some(expand_all) = right_root.as_expand_all() {
            let mut new_expand = expand_all.clone();

            // Add the left plan (previous edge expansion) as an input dependency
            new_expand.add_input(left_root.clone());

            // Set the input_var to the left_dst_alias so ExpandAllExecutor can find the input
            // The previous edge expansion's output will be stored in ExecutionContext under this variable name
            new_expand.set_input_var(left_dst_alias.to_string());

            // Set the output_var to help subsequent operations find the result
            new_expand.set_output_var(format!("expand_{}", new_expand.id()));

            Ok(SubPlan {
                root: Some(new_expand.into_enum()),
                tail: left_plan.tail.or(right_plan.tail),
            })
        } else {
            // If not an ExpandAllNode, fall back to cross_join_plans
            self.cross_join_plans(left_plan, right_plan)
        }
    }

    fn plan_node_pattern(&self, space_id: u64, space_name: &str) -> Result<SubPlan, PlannerError> {
        let scan_node = ScanVerticesNode::new(space_id, space_name);
        Ok(SubPlan::from_root(scan_node.into_enum()))
    }

    fn plan_match_delete(
        &self,
        input_plan: SubPlan,
        delete_clause: &crate::query::parser::ast::stmt::MatchDeleteClause,
        space_name: &str,
        match_stmt: &crate::query::parser::ast::stmt::MatchStmt,
    ) -> Result<SubPlan, PlannerError> {
        use crate::query::planning::plan::core::next_node_id;
        use crate::query::planning::plan::core::nodes::data_modification::delete_nodes::{
            PipeDeleteEdgesNode, PipeDeleteVerticesNode,
        };
        use crate::query::planning::plan::core::nodes::data_modification::info::VertexDeleteInfo;

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        let delete_node = match &delete_clause.target {
            crate::query::parser::ast::stmt::MatchDeleteTarget::Vertices(vertex_exprs) => {
                let info = VertexDeleteInfo {
                    space_name: space_name.to_string(),
                    vertex_ids: vertex_exprs.clone(),
                    with_edge: delete_clause.with_edge,
                    condition: None,
                };
                PipeDeleteVerticesNode::new(next_node_id(), info, input_node.clone()).into_enum()
            }
            crate::query::parser::ast::stmt::MatchDeleteTarget::Edges(edge_exprs) => {
                use crate::query::planning::plan::core::nodes::data_modification::info::EdgeDeleteInfo;

                let edges: Vec<_> = edge_exprs
                    .iter()
                    .map(|e| (e.clone(), e.clone(), None))
                    .collect();

                let info = EdgeDeleteInfo {
                    space_name: space_name.to_string(),
                    edges,
                    edge_type: None,
                    condition: None,
                };
                PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
            }
            crate::query::parser::ast::stmt::MatchDeleteTarget::EdgeRefs(edge_refs) => {
                use crate::query::planning::plan::core::nodes::data_modification::info::EdgeDeleteInfo;

                let edges = edge_refs.clone();
                let edge_type = extract_edge_type_from_patterns(&match_stmt.patterns);

                let info = EdgeDeleteInfo {
                    space_name: space_name.to_string(),
                    edges,
                    edge_type,
                    condition: None,
                };
                PipeDeleteEdgesNode::new(next_node_id(), info, input_node.clone()).into_enum()
            }
        };

        Ok(SubPlan::new(Some(delete_node), input_plan.tail))
    }

    fn has_where_clause(&self, stmt: &crate::query::parser::ast::Stmt) -> bool {
        match stmt {
            crate::query::parser::ast::Stmt::Match(match_stmt) => match_stmt.where_clause.is_some(),
            _ => false,
        }
    }

    fn has_return_clause(&self, stmt: &crate::query::parser::ast::Stmt) -> bool {
        match stmt {
            crate::query::parser::ast::Stmt::Match(match_stmt) => {
                match_stmt.return_clause.is_some()
            }
            _ => false,
        }
    }

    fn has_order_by_clause(&self, stmt: &crate::query::parser::ast::Stmt) -> bool {
        match stmt {
            crate::query::parser::ast::Stmt::Match(match_stmt) => match_stmt.order_by.is_some(),
            _ => false,
        }
    }

    fn has_pagination(&self, stmt: &crate::query::parser::ast::Stmt) -> bool {
        match stmt {
            crate::query::parser::ast::Stmt::Match(match_stmt) => {
                match_stmt.limit.is_some() || match_stmt.skip.is_some()
            }
            _ => false,
        }
    }

    /// Planning Alternative Paths Pattern
    ///
    /// Convert multiple path options into a union operation.
    /// Example: (a)-[:KNOWS|WORKS_WITH]->(b) denotes either KNOWS or WORKS_WITH.
    fn plan_alternative_patterns(
        &self,
        patterns: &[Pattern],
        space_id: u64,
        space_name: &str,
        _prev_alias: Option<&str>,
        validation_info: &ValidationInfo,
        qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        if patterns.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "The alternative path cannot be empty".to_string(),
            ));
        }

        let mut plan =
            self.plan_pattern(&patterns[0], space_id, space_name, validation_info, qctx)?;

        for pattern in patterns.iter().skip(1) {
            let pattern_plan =
                self.plan_pattern(pattern, space_id, space_name, validation_info, qctx)?;
            plan = self.union_plans(plan, pattern_plan)?;
        }

        Ok(plan)
    }

    /// Planning a single pattern (node, edge, or path)
    fn plan_pattern(
        &self,
        pattern: &Pattern,
        space_id: u64,
        space_name: &str,
        validation_info: &ValidationInfo,
        _qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        match pattern {
            Pattern::Node(node) => self.plan_pattern_node(node, space_id, space_name),
            Pattern::Edge(edge) => self.plan_pattern_edge(edge, space_id, space_name),
            Pattern::Path(_) => {
                self.plan_path_pattern(pattern, space_id, space_name, validation_info, _qctx)
            }
            Pattern::Variable(var) => self.plan_variable_pattern(var, space_id, validation_info),
        }
    }

    /// Planning Variable Pattern
    ///
    /// The variable pattern references a previously defined variable, using an ArgumentNode as the data source.
    /// Refer to the implementation of VariableVertexIdSeek in nebula-graph.
    ///
    /// # Design Specifications
    ///
    /// The variable pattern is used to reference variables that were defined in a previous MATCH clause, for example:
    /// ```cypher
    /// MATCH (a), a RETURN a
    /// ```
    /// In this example, the second “a” represents a variable pattern that refers to the node defined by the first “(a)”.
    ///
    /// # Execution Process
    ///
    /// 1. Create an `ArgumentNode` to indicate that a variable needs to be retrieved from the execution context.
    /// 2. During the execution phase, the ArgumentExecutor retrieves the variable values from the ExecutionContext.
    /// 3. If the variable does not exist, return an execution error.
    fn plan_variable_pattern(
        &self,
        var: &crate::query::parser::ast::pattern::VariablePattern,
        _space_id: u64,
        validation_info: &ValidationInfo,
    ) -> Result<SubPlan, PlannerError> {
        // Use the alias_map of ValidationInfo to verify whether the variable exists.
        if !validation_info.alias_map.contains_key(&var.name) {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Variable '{}' undefined",
                var.name
            )));
        }

        // Create an ArgumentNode to reference the variable.
        // The `ArgumentNode` represents data input from external variables, which is used for subqueries or schema references.
        let argument_node = ArgumentNode::new(0, &var.name);

        // Create a SubPlan that contains only the ArgumentNode.
        let sub_plan = SubPlan::from_root(argument_node.into_enum());

        Ok(sub_plan)
    }

    /// Merge the two plans into a union.
    fn union_plans(&self, left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        // Create an union node to remove duplicates.
        let union_node = UnionNode::new(
            left_root.clone(),
            right_root.clone(),
            true, // `distinct = true` – to remove duplicates.
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Concatenation operation failed: {}", e))
        })?;

        Ok(SubPlan {
            root: Some(union_node.into_enum()),
            tail: left.tail.or(right.tail),
        })
    }

    /// Planning of optional path elements
    ///
    /// Use a left join to achieve an optional match, retaining all data from the left side.
    /// Example: (a)-[:KNOWS]->(b)? means that the KNOWS relation is optional.
    fn plan_optional_element(
        &self,
        element: &PathElement,
        space_id: u64,
        space_name: &str,
        _prev_alias: Option<&str>,
        _validation_info: &ValidationInfo,
        _qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let opt_plan = match element {
            PathElement::Node(node) => self.plan_pattern_node(node, space_id, space_name)?,
            PathElement::Edge(edge) => self.plan_pattern_edge(edge, space_id, space_name)?,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Optional paths do not support nested complex patterns".to_string(),
                ));
            }
        };

        Ok(opt_plan)
    }

    /// The left join connects two plans.
    fn left_join_plans(&self, left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        // Create a left join node.
        let join_node = LeftJoinNode::new(
            left_root.clone(),
            right_root.clone(),
            vec![], // hash_keys
            vec![], // probe_keys
        )
        .map_err(|e| PlannerError::JoinFailed(format!("Left connection failed: {}", e)))?;

        Ok(SubPlan {
            root: Some(join_node.into_enum()),
            tail: left.tail.or(right.tail),
        })
    }

    /// Planning for repeated path elements
    ///
    /// Implementing variable-length paths using loop nodes
    /// Example: (a)-[:KNOWS*1..3]->(b) represents the 1 to 3 jump KNOWS relation.
    fn plan_repeated_element(
        &self,
        element: &PathElement,
        rep_type: RepetitionType,
        space_id: u64,
        space_name: &str,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<SubPlan, PlannerError> {
        let base_plan = match element {
            PathElement::Node(node) => self.plan_pattern_node(node, space_id, space_name)?,
            PathElement::Edge(edge) => self.plan_pattern_edge(edge, space_id, space_name)?,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Repeated paths do not support nested complex patterns".to_string(),
                ));
            }
        };

        // Determine the loop condition based on the type of repetition.
        let condition_str = match rep_type {
            RepetitionType::ZeroOrMore => "loop_count >= 0".to_string(),
            RepetitionType::OneOrMore => "loop_count >= 1".to_string(),
            RepetitionType::ZeroOrOne => "loop_count <= 1".to_string(),
            RepetitionType::Exactly(n) => format!("loop_count == {}", n),
            RepetitionType::Range(min, max) => {
                format!("loop_count >= {} && loop_count <= {}", min, max)
            }
        };

        // Create a loop condition expression
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable(condition_str),
        );
        let id = expr_context.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_context.clone());

        // Create a loop node
        let mut loop_node = LoopNode::new(-1, ctx_expr);

        // Setting up the loop body
        if let Some(base_root) = base_plan.root {
            loop_node.set_body(base_root);
        }

        Ok(SubPlan {
            root: Some(loop_node.into_enum()),
            tail: base_plan.tail,
        })
    }

    /// Constructing tag filtering expressions
    ///
    /// Convert the list of node labels into an expression that can be used to filter nodes with the specified labels.
    /// 例如: 标签 ["Person", "Actor"] 转换为: labels(n) CONTAINS "Person" AND labels(n) CONTAINS "Actor"
    fn build_label_filter_expression(
        variable: &Option<String>,
        labels: &[String],
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> ContextualExpression {
        let var_name = variable.as_deref().unwrap_or("n");
        let var_expr = crate::core::Expression::variable(var_name);

        let ctx = expr_context.clone();

        // 创建 labels() 函数调用表达式
        let labels_func = crate::core::Expression::function("labels", vec![var_expr]);

        let expr = if labels.len() == 1 {
            // 单个标签: labels(n) CONTAINS "label"
            let label_literal = crate::core::Expression::literal(labels[0].clone());
            crate::core::Expression::function("contains", vec![labels_func, label_literal])
        } else {
            // 多个标签: labels(n) CONTAINS "label1" AND labels(n) CONTAINS "label2" AND ...
            let first_label = crate::core::Expression::literal(labels[0].clone());
            let first_condition = crate::core::Expression::function(
                "contains",
                vec![labels_func.clone(), first_label],
            );

            labels.iter().skip(1).fold(first_condition, |acc, label| {
                let label_literal = crate::core::Expression::literal(label.clone());
                let condition = crate::core::Expression::function(
                    "contains",
                    vec![labels_func.clone(), label_literal],
                );
                crate::core::Expression::binary(
                    acc,
                    crate::core::types::operators::BinaryOperator::And,
                    condition,
                )
            })
        };

        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        ContextualExpression::new(id, ctx)
    }

    /// Convert property map to filter expression
    ///
    /// Converts a map literal like `{name: 'Alice', age: 30}` into a conjunction of equality comparisons
    /// like `v.name = 'Alice' AND v.age = 30`.
    fn convert_properties_to_filter(
        var_name: &str,
        props: &ContextualExpression,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Option<ContextualExpression> {
        use crate::core::types::operators::BinaryOperator;

        let props_expr = props.expression()?.inner().clone();

        if let crate::core::Expression::Map(pairs) = props_expr {
            if pairs.is_empty() {
                return None;
            }

            let var_expr = crate::core::Expression::variable(var_name);

            let conditions: Vec<crate::core::Expression> = pairs
                .into_iter()
                .map(|(key, value)| {
                    let prop_access = crate::core::Expression::property(var_expr.clone(), key);
                    crate::core::Expression::binary(prop_access, BinaryOperator::Equal, value)
                })
                .collect();

            let combined = conditions.into_iter().reduce(|acc, cond| {
                crate::core::Expression::binary(acc, BinaryOperator::And, cond)
            })?;

            let expr_meta = crate::core::types::expr::ExpressionMeta::new(combined);
            let id = expr_context.register_expression(expr_meta);
            Some(ContextualExpression::new(id, expr_context.clone()))
        } else {
            None
        }
    }
}

fn extract_edge_type_from_patterns(patterns: &[Pattern]) -> Option<String> {
    for pattern in patterns {
        if let Pattern::Path(path_pattern) = pattern {
            for element in &path_pattern.elements {
                if let PathElement::Edge(edge_pattern) = element {
                    if let Some(edge_type) = edge_pattern.edge_types.first() {
                        return Some(edge_type.clone());
                    }
                }
            }
        }
    }
    None
}

fn extract_distinct_flag_from_stmt(stmt: &crate::query::parser::ast::Stmt) -> bool {
    if let crate::query::parser::ast::Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.distinct;
        }
    }
    false
}
