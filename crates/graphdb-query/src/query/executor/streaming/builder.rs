//! Builder for StreamingExecutor from ExecutionPlan
//!
//! This module provides utilities to construct streaming executors
//! from the execution plan. Supports automatic conversion of plan nodes
//! to streaming executor operators.

use super::executor::StreamingExecutor;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::{AggregateFunction, BinaryOperator};
use crate::core::Value;
use crate::query::core::NodeType;
use crate::query::executor::base::{ExecutionContext, MemoryTracker};
use crate::query::parser::ast::fulltext::FulltextQueryExpr;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, FulltextManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
    UserManageNode, VectorManageNode,
};

/// Builder for constructing StreamingExecutor instances
pub struct StreamingExecutorBuilder;

impl StreamingExecutorBuilder {
    /// Create a simple scan executor for testing/basic use
    pub fn build_simple_scan(rows: Vec<Vec<Value>>) -> Result<StreamingExecutor, QueryError> {
        Self::build_simple_scan_with_col_names(rows, vec![])
    }

    /// Create a simple scan with explicit column names (avoids col_N inference)
    pub fn build_simple_scan_with_col_names(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    ) -> Result<StreamingExecutor, QueryError> {
        if rows.is_empty() {
            return Ok(StreamingExecutor::ScanVertices {
                partition_id: 0,
                buffer: vec![],
                current_index: 0,
                col_names,
                plan_node_id: 0,
                runtime: None,
            });
        }

        Ok(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: rows,
            current_index: 0,
            col_names,
            plan_node_id: 0,
            runtime: None,
        })
    }

    /// Convert a single plan node to a streaming executor
    /// Supports: ScanVertices, ScanEdges, Filter, Project, Limit, Aggregate, and Join operations
    pub fn from_plan_node(
        node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        match node {
            // ======== Scan Operations ========
            PlanNodeEnum::ScanVertices(scan_node) => {
                let limit = scan_node
                    .limit()
                    .and_then(|value| (value >= 0).then_some(value as usize));
                let col_names = scan_node.col_names().to_vec();
                let executor = StreamingExecutor::StorageScanVertices {
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    limit,
                    cursor: None,
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names,
                    plan_node_id: node.id(),
                    runtime: None,
                };

                Ok(executor)
            }

            PlanNodeEnum::ScanEdges(scan_node) => {
                let limit = scan_node
                    .limit()
                    .and_then(|value| (value >= 0).then_some(value as usize));
                let col_names = scan_node.col_names().to_vec();
                // Phase 3: extract optional edge_type from the plan node.
                let edge_type = scan_node.edge_type().map(|s| s.to_string());
                let executor = StreamingExecutor::StorageScanEdges {
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    limit,
                    edge_type,
                    cursor: None,
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names,
                    plan_node_id: node.id(),
                    runtime: None,
                };

                Ok(executor)
            }

            // ======== Filter Operation ========
            PlanNodeEnum::Filter(filter_node) => {
                let input_plan = filter_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let condition = filter_node.condition();
                let predicate = Self::contextual_to_expression(condition)?;

                Ok(StreamingExecutor::Filter {
                    input: Box::new(input_executor),
                    predicate,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Project Operation ========
            PlanNodeEnum::Project(project_node) => {
                let input_plan = project_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let columns = project_node.columns();
                let output_expressions = Self::yield_columns_to_expressions(columns)?;

                Ok(StreamingExecutor::Project {
                    input: Box::new(input_executor),
                    output_expressions,
                    output_col_names: project_node.col_names().to_vec(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Limit Operation ========
            PlanNodeEnum::Limit(limit_node) => {
                let input_plan = limit_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let count = limit_node.count();
                if count < 0 {
                    return Err(QueryError::execution(
                        "Limit count must be non-negative".to_string(),
                    ));
                }

                Ok(StreamingExecutor::Limit {
                    input: Box::new(input_executor),
                    limit: count as u32,
                    consumed: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Aggregate Operation ========
            PlanNodeEnum::Aggregate(agg_node) => {
                let input_plan = agg_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let group_keys = agg_node.group_keys();
                // Convert group keys to expressions (as variable references)
                let group_by_expressions: Vec<Expression> = group_keys
                    .iter()
                    .map(|key| Expression::Variable(key.clone()))
                    .collect();

                let agg_functions = agg_node.aggregation_functions();

                // Pair each aggregation function with an expression
                let aggregate_functions: Vec<(AggregateFunction, Expression)> = agg_functions
                    .iter()
                    .map(|func| {
                        let expr = match func {
                            AggregateFunction::Count(Some(field)) => {
                                Expression::Variable(field.clone())
                            }
                            AggregateFunction::Sum(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Avg(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Min(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Max(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Collect(field) => {
                                Expression::Variable(field.clone())
                            }
                            AggregateFunction::Count(None) => Expression::Literal(Value::Int(1)),
                            _ => Expression::Literal(Value::Int(1)),
                        };
                        (func.clone(), expr)
                    })
                    .collect();

                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                Ok(StreamingExecutor::Aggregate {
                    input: Box::new(input_executor),
                    group_by_expressions,
                    aggregate_functions,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                    memory_tracker,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Join Operations ========
            PlanNodeEnum::InnerJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;

                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                Ok(StreamingExecutor::InnerJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    build_side_tuples: vec![],
                    left_consumed: false,
                    opened: false,
                    memory_tracker,
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::LeftJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;

                Ok(StreamingExecutor::LeftJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    build_side_tuples: vec![],
                    left_consumed: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::CrossJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                Ok(StreamingExecutor::CrossJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    all_left_rows: vec![],
                    all_right_rows: vec![],
                    left_consumed: false,
                    right_consumed: false,
                    opened: false,
                    memory_tracker,
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Set Operations ========
            PlanNodeEnum::Union(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.union_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                Ok(StreamingExecutor::Union {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    seen_rows: std::collections::HashSet::new(),
                    left_consumed: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Intersect(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.intersect_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                Ok(StreamingExecutor::Intersect {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    left_rows: Vec::new(),
                    right_rows: std::collections::HashSet::new(),
                    left_buffered: false,
                    right_buffered: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Minus(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.minus_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;

                Ok(StreamingExecutor::Except {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    exclude_rows: std::collections::HashSet::new(),
                    right_buffered: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Other Operations (partially supported) ========
            PlanNodeEnum::Sort(sort_node) => {
                let input_plan = sort_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let sort_items = sort_node.sort_items();
                if sort_items.is_empty() {
                    // No sort items: pass through
                    Ok(input_executor)
                } else {
                    let (sort_expressions, sort_directions) =
                        Self::sort_items_to_expressions(sort_items)?;

                    let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                    Ok(StreamingExecutor::Sort {
                        input: Box::new(input_executor),
                        sort_expressions,
                        sort_directions,
                        all_rows: vec![],
                        row_iter: None,
                        opened: false,
                        memory_tracker,
                        plan_node_id: node.id(),
                        runtime: None,
                    })
                }
            }

            PlanNodeEnum::Dedup(dedup_node) => {
                let input_plan = dedup_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                Ok(StreamingExecutor::Distinct {
                    input: Box::new(input_executor),
                    seen_rows: std::collections::HashSet::new(),
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Management/DDL Operations ========
            PlanNodeEnum::SpaceManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let space_name = Self::extract_space_manage_name(manage_node);
                Ok(StreamingExecutor::SpaceManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    action,
                    space_name,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::TagManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let tag_name = Self::extract_tag_manage_name(manage_node);
                let properties = Self::extract_tag_manage_properties(manage_node);
                Ok(StreamingExecutor::TagManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    action,
                    tag_name,
                    properties,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::EdgeManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let edge_type = Self::extract_edge_manage_name(manage_node);
                let properties = Self::extract_edge_manage_properties(manage_node);
                Ok(StreamingExecutor::EdgeManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    action,
                    edge_type,
                    properties,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::IndexManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let index_name = Self::extract_index_manage_name(manage_node);
                Ok(StreamingExecutor::IndexManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    action,
                    index_name,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::UserManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let username = Self::extract_user_manage_name(manage_node);
                Ok(StreamingExecutor::UserManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    action,
                    username,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::FulltextManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let index_name = Self::extract_fulltext_manage_name(manage_node);
                use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode::*;
                let (tag_name, field_name, space_id) = match manage_node {
                    Create(n) => (
                        Some(n.schema_name.clone()),
                        Some(
                            n.fields
                                .first()
                                .map(|f| f.field_name.clone())
                                .unwrap_or_default(),
                        ),
                        Some(n.space_id),
                    ),
                    _ => (None, None, None),
                };
                #[cfg(feature = "fulltext-search")]
                let fulltext_manager = context.fulltext_manager.clone();
                Ok(StreamingExecutor::FulltextManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id: space_id.unwrap_or(0),
                    action,
                    index_name,
                    tag_name,
                    field_name,
                    #[cfg(feature = "fulltext-search")]
                    fulltext_manager,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::VectorManage(manage_node) => {
                let action = manage_node.node_type_id().to_string();
                let index_name = Self::extract_vector_manage_name(manage_node);
                use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode::*;
                let (tag_name, field_name, space_id) = match manage_node {
                    Create(n) => (
                        Some(n.tag_name.clone()),
                        Some(n.field_name.clone()),
                        Some(n.space_id),
                    ),
                    _ => (None, None, None),
                };
                Ok(StreamingExecutor::VectorManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id: space_id.unwrap_or(0),
                    action,
                    index_name,
                    tag_name,
                    field_name,
                    #[cfg(feature = "qdrant")]
                    vector_coordinator: context.vector_coordinator.clone(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Graph Traversal Operations ========
            PlanNodeEnum::Expand(expand_node) => {
                let input_plan = expand_node
                    .inputs()
                    .first()
                    .ok_or_else(|| QueryError::execution("Expand requires an input".to_string()))?;
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let edge_types = expand_node.edge_types().to_vec();
                let direction = expand_node.direction();

                Ok(StreamingExecutor::Expand {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    edge_types,
                    direction,
                    filter_expr: expand_node
                        .filter()
                        .map(Self::contextual_to_expression)
                        .transpose()?,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::ExpandAll(expand_all_node) => {
                let input_plan = expand_all_node.inputs().first().ok_or_else(|| {
                    QueryError::execution("ExpandAll requires an input".to_string())
                })?;
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let edge_types = expand_all_node.edge_types().to_vec();
                let direction = match expand_all_node.direction().to_lowercase().as_str() {
                    "out" | "outgoing" => crate::core::EdgeDirection::Out,
                    "in" | "incoming" => crate::core::EdgeDirection::In,
                    _ => crate::core::EdgeDirection::Both,
                };

                Ok(StreamingExecutor::ExpandAll {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    edge_types,
                    direction,
                    filter_expr: expand_all_node
                        .filter()
                        .map(Self::contextual_to_expression)
                        .transpose()?,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Traverse(traverse_node) => {
                let input_plan = traverse_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;

                let edge_types = traverse_node.edge_types().to_vec();
                let direction = traverse_node.direction();
                let min_depth = traverse_node.min_steps();
                let max_depth = traverse_node.max_steps();

                Ok(StreamingExecutor::Traverse {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    edge_types,
                    direction,
                    min_depth,
                    max_depth,
                    filter_expr: traverse_node
                        .e_filter()
                        .or_else(|| traverse_node.v_filter())
                        .map(Self::contextual_to_expression)
                        .transpose()?,
                    visited: std::collections::HashSet::new(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Data Modification Operations ========
            PlanNodeEnum::InsertVertices(insert_node) => {
                let mut rows = Vec::new();
                let prop_names: Vec<String> = insert_node
                    .tags()
                    .iter()
                    .flat_map(|tag| tag.prop_names.iter().cloned())
                    .collect();
                let mut scan_col_names = vec!["vid".to_string()];
                scan_col_names.extend(prop_names.clone());
                let mut vertex_properties =
                    vec![("vid".to_string(), Expression::Variable("vid".to_string()))];
                for prop_name in &prop_names {
                    vertex_properties.push((
                        prop_name.clone(),
                        Expression::Variable(prop_name.clone()),
                    ));
                }
                for (vid_expr, tag_values) in insert_node.values() {
                    let mut row = vec![Self::contextual_to_value(vid_expr)?];
                    for values in tag_values {
                        for value_expr in values {
                            row.push(Self::contextual_to_value(value_expr)?);
                        }
                    }
                    rows.push(row);
                }
                Ok(StreamingExecutor::InsertVertices {
                    input: Box::new(Self::build_simple_scan_with_col_names(rows, scan_col_names)?),
                    storage: context.storage.clone(),
                    space_name: insert_node.space_name().to_string(),
                    vertex_properties,
                    tags: insert_node.tag_names(),
                    rows_inserted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::InsertEdges(insert_node) => {
                let mut rows = Vec::new();
                let prop_names = insert_node.prop_names();
                let mut scan_col_names = vec!["src".to_string(), "dst".to_string(), "rank".to_string()];
                scan_col_names.extend(prop_names.iter().cloned());
                for (src, dst, rank, props) in insert_node.edges() {
                    let mut row = vec![
                        Self::contextual_to_value(src)?,
                        Self::contextual_to_value(dst)?,
                    ];
                    row.push(match rank {
                        Some(rank_expr) => Self::contextual_to_value(rank_expr)?,
                        None => Value::BigInt(0),
                    });
                    for prop in props {
                        row.push(Self::contextual_to_value(prop)?);
                    }
                    rows.push(row);
                }
                let edge_properties = prop_names
                    .iter()
                    .map(|prop| {
                        (prop.clone(), Expression::Variable(prop.clone()))
                    })
                    .collect();
                Ok(StreamingExecutor::InsertEdges {
                    input: Box::new(Self::build_simple_scan_with_col_names(rows, scan_col_names)?),
                    storage: context.storage.clone(),
                    space_name: insert_node.space_name().to_string(),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    edge_type: insert_node.edge_name().to_string(),
                    edge_properties,
                    rows_inserted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::UpdateVertices(update_node) => {
                let mut rows = Vec::new();
                let mut updates = Vec::new();
                for update in update_node.updates() {
                    rows.push(vec![Self::contextual_to_value(&update.vertex_id)?]);
                    for (name, expr) in &update.properties {
                        updates.push((name.clone(), Self::contextual_to_expression(expr)?));
                    }
                }
                Ok(StreamingExecutor::UpdateVertices {
                    input: Box::new(Self::build_simple_scan(rows)?),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    updates,
                    rows_updated: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Update(update_node) => {
                use crate::query::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
                match update_node.info() {
                    UpdateTargetType::Vertex(vinfo) => {
                        let updates: Vec<(String, Expression)> = vinfo
                            .properties
                            .iter()
                            .filter_map(|(k, v)| {
                                v.get_expression()
                                    .map(|e| (k.clone(), e))
                            })
                            .collect();
                        let row = vec![
                            vinfo.vertex_id
                                .constant_value()
                                .unwrap_or(Value::Null(crate::core::NullType::Null)),
                        ];
                        Ok(StreamingExecutor::UpdateVertices {
                            input: Box::new(Self::build_simple_scan_with_col_names(
                                vec![row],
                                vec!["vid".to_string()],
                            )?),
                            storage: context.storage.clone(),
                            space_name: vinfo.space_name.clone(),
                            updates,
                            rows_updated: 0,
                            opened: false,
                            plan_node_id: node.id(),
                            runtime: None,
                        })
                    }
                    UpdateTargetType::Edge(einfo) => {
                        let updates: Vec<(String, Expression)> = einfo
                            .properties
                            .iter()
                            .filter_map(|(k, v)| {
                                v.get_expression()
                                    .map(|e| (k.clone(), e))
                            })
                            .collect();
                        let src_val = einfo
                            .src
                            .constant_value()
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        let dst_val = einfo
                            .dst
                            .constant_value()
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        Ok(StreamingExecutor::UpdateEdges {
                            input: Box::new(Self::build_simple_scan_with_col_names(
                                vec![vec![src_val, dst_val]],
                                vec!["src".to_string(), "dst".to_string()],
                            )?),
                            storage: context.storage.clone(),
                            space_name: einfo.space_name.clone(),
                            src_col: "src".to_string(),
                            dst_col: "dst".to_string(),
                            edge_type: einfo.edge_type.clone().unwrap_or_default(),
                            updates,
                            rows_updated: 0,
                            opened: false,
                            plan_node_id: node.id(),
                            runtime: None,
                        })
                    }
                }
            }

            PlanNodeEnum::UpdateEdges(update_node) => {
                let updates: Vec<(String, Expression)> = update_node
                    .updates()
                    .iter()
                    .flat_map(|u| {
                        u.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone().into_expression()))
                    })
                    .collect();
                let src_col = update_node
                    .updates()
                    .first()
                    .and_then(|u| u.src.as_variable())
                    .unwrap_or_else(|| "src".to_string());
                let dst_col = update_node
                    .updates()
                    .first()
                    .and_then(|u| u.dst.as_variable())
                    .unwrap_or_else(|| "dst".to_string());
                let edge_type = update_node
                    .updates()
                    .first()
                    .and_then(|u| u.edge_type.clone())
                    .unwrap_or_default();
                Ok(StreamingExecutor::UpdateEdges {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    src_col,
                    dst_col,
                    edge_type,
                    updates,
                    rows_updated: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::DeleteVertices(delete_node) => {
                let rows = delete_node
                    .vertex_ids()
                    .iter()
                    .map(|id| Self::contextual_to_value(id).map(|value| vec![value]))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(StreamingExecutor::DeleteVertices {
                    input: Box::new(Self::build_simple_scan_with_col_names(rows, vec!["vid".to_string()])?),
                    storage: context.storage.clone(),
                    space_name: delete_node.space_name().to_string(),
                    vertex_id_col: "vid".to_string(),
                    rows_deleted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::DeleteEdges(delete_node) => {
                let rows = delete_node
                    .edges()
                    .iter()
                    .map(|(src, dst, _rank)| {
                        Ok(vec![
                            Self::contextual_to_value(src)?,
                            Self::contextual_to_value(dst)?,
                        ])
                    })
                    .collect::<Result<Vec<_>, QueryError>>()?;
                Ok(StreamingExecutor::DeleteEdges {
                    input: Box::new(Self::build_simple_scan_with_col_names(rows, vec!["src".to_string(), "dst".to_string()])?),
                    storage: context.storage.clone(),
                    space_name: delete_node.space_name().to_string(),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    rows_deleted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::PipeDeleteVertices(delete_node) => {
                let input_plan = delete_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::PipeDeleteVertices {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    vertex_id_col: "vid".to_string(),
                    rows_deleted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::PipeDeleteEdges(delete_node) => {
                let input_plan = delete_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::PipeDeleteEdges {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    rows_deleted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Data Access Operations ========
            PlanNodeEnum::GetVertices(get_node) => {
                let vertex_ids = get_node.src_ref().constant_value().map(|v| vec![v]);
                Ok(StreamingExecutor::GetVertices {
                    storage: context.storage.clone(),
                    space_name: get_node.space_name().to_string(),
                    vertex_ids,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::GetEdges(get_node) => Ok(StreamingExecutor::GetEdges {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(get_node.edge_type().to_string()),
                src: Some(get_node.src().to_string()),
                dst: Some(get_node.dst().to_string()),
                rank: 0,
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            PlanNodeEnum::GetNeighbors(get_node) => Ok(StreamingExecutor::GetNeighbors {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                direction: get_node.direction().to_string(),
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== Search Operations ========
            PlanNodeEnum::FulltextSearch(search_node) => {
                let query_str = Self::fulltext_query_to_string(&search_node.query);
                let space_id = context.current_space_id().unwrap_or(0);
                #[cfg(feature = "fulltext-search")]
                let fulltext_manager = context.fulltext_manager.clone();
                Ok(StreamingExecutor::FulltextSearch {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id,
                    index_name: search_node.index_name.clone(),
                    search_query: query_str,
                    tag_name: search_node.tag_name.clone(),
                    field_name: search_node.field_name.clone(),
                    #[cfg(feature = "fulltext-search")]
                    fulltext_manager,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::FulltextLookup(lookup_node) => {
                let space_id = context.current_space_id().unwrap_or(0);
                #[cfg(feature = "fulltext-search")]
                let fulltext_manager = context.fulltext_manager.clone();
                Ok(StreamingExecutor::FulltextLookup {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id,
                    index_name: lookup_node.index_name.clone(),
                    search_query: lookup_node.query.clone(),
                    tag_name: lookup_node.tag_name.clone(),
                    field_name: lookup_node.field_name.clone(),
                    #[cfg(feature = "fulltext-search")]
                    fulltext_manager,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::MatchFulltext(match_node) => {
                let condition_str = Self::fulltext_match_to_string(&match_node.fulltext_condition);
                #[cfg(feature = "fulltext-search")]
                let fulltext_manager = context.fulltext_manager.clone();
                Ok(StreamingExecutor::MatchFulltext {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    match_expr: Expression::Literal(Value::String(condition_str)),
                    match_field: Some(match_node.field_name.clone()),
                    tag_name: match_node.tag_name.clone(),
                    field_name: match_node.field_name.clone(),
                    #[cfg(feature = "fulltext-search")]
                    fulltext_manager,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(search_node) => {
                let query_vec = Self::vector_query_to_vec(&search_node.query);
                Ok(StreamingExecutor::VectorSearch {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id: search_node.space_id,
                    index_name: search_node.index_name.clone(),
                    query_vector: query_vec,
                    top_k: search_node.limit as u32,
                    tag_name: search_node.tag_name.clone(),
                    field_name: search_node.field_name.clone(),
                    vector_coordinator: context.vector_coordinator.clone(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorLookup(lookup_node) => Ok(StreamingExecutor::VectorLookup {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                index_name: lookup_node.index_name.clone(),
                lookup_key: Expression::Literal(Value::String(
                    lookup_node.query.query_data.clone(),
                )),
                vector_coordinator: context.vector_coordinator.clone(),
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            PlanNodeEnum::Window(window_node) => {
                let input_plan = window_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let window_functions = window_node.window_functions();

                let mut window_exprs = Vec::new();
                let mut partition_by_exprs = Vec::new();
                let mut order_by_exprs = Vec::new();
                let mut order_by_directions = Vec::new();

                for wf in window_functions {
                    let window_expr = Expression::WindowFunction {
                        name: wf.name.clone(),
                        args: wf.args.clone(),
                        over_partition_by: wf.partition_by.clone(),
                        over_order_by: wf.order_by.clone(),
                        over_order_desc: wf.order_desc.clone(),
                    };
                    window_exprs.push(window_expr);

                    if partition_by_exprs.is_empty() {
                        partition_by_exprs = wf.partition_by.clone();
                    }
                    if order_by_exprs.is_empty() {
                        order_by_exprs = wf.order_by.clone();
                        order_by_directions = wf
                            .order_desc
                            .iter()
                            .map(|&desc| {
                                if desc {
                                    super::executor::SortDirection::Descending
                                } else {
                                    super::executor::SortDirection::Ascending
                                }
                            })
                            .collect();
                    }
                }

                Ok(StreamingExecutor::WindowFunction {
                    input: Box::new(input_executor),
                    window_exprs,
                    partition_by_exprs,
                    order_by_exprs,
                    order_by_directions,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Remove(remove_node) => {
                let input_plan = remove_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let remove_items = remove_node.remove_items();
                let columns_to_remove: Vec<String> =
                    remove_items.iter().map(|(col, _)| col.clone()).collect();

                Ok(StreamingExecutor::Remove {
                    input: Box::new(input_executor),
                    columns_to_remove,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::DeleteTags(delete_tags_node) => {
                let space_name = delete_tags_node.space_name().to_string();
                let tag_names = delete_tags_node.tag_names().to_vec();
                let vertex_ids: Vec<Value> = delete_tags_node
                    .vertex_ids()
                    .iter()
                    .filter_map(|expr| {
                        expr.get_expression().and_then(|e| {
                            if let Expression::Literal(v) = e {
                                Some(v.clone())
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                Ok(StreamingExecutor::DeleteTags {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name,
                    tag_names,
                    vertex_ids: Some(vertex_ids),
                    rows_deleted: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorMatch(match_node) => {
                let query_vec = Self::vector_query_to_vec(&match_node.query);
                Ok(StreamingExecutor::VectorMatch {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    pattern: match_node.pattern.clone(),
                    field: match_node.field.clone(),
                    query_vector: query_vec,
                    threshold: match_node.threshold,
                    tag_name: match_node.tag_name.clone(),
                    field_name: match_node.field_name.clone(),
                    space_id: match_node.space_id,
                    vector_coordinator: context.vector_coordinator.clone(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            PlanNodeEnum::Commit(_) => Ok(StreamingExecutor::Commit {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                transaction_id: None,
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            PlanNodeEnum::Rollback(_) => Ok(StreamingExecutor::Rollback {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                transaction_id: None,
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== Start / Argument (leaf nodes) ========
            PlanNodeEnum::Start(_) => Ok(StreamingExecutor::Start { opened: false, plan_node_id: node.id(), runtime: None }),

            PlanNodeEnum::Argument(_) => Ok(StreamingExecutor::Argument { opened: false, plan_node_id: node.id(), runtime: None }),

            // ======== Index & EdgeIndex Scan ========
            PlanNodeEnum::EdgeIndexScan(scan_node) => Ok(StreamingExecutor::EdgeIndexScan {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(scan_node.edge_type().to_string()),
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            PlanNodeEnum::IndexScan(scan_node) => Ok(StreamingExecutor::IndexScan {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                index_name: Some(scan_node.index_name().to_string()),
                index_value: None,
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== Sample ========
            PlanNodeEnum::Sample(sample_node) => {
                let input_plan = sample_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let count = if sample_node.count() > 0 {
                    sample_node.count() as u64
                } else {
                    return Err(QueryError::execution(
                        "Sample count must be positive".to_string(),
                    ));
                };
                Ok(StreamingExecutor::Sample {
                    input: Box::new(input_executor),
                    count,
                    consumed: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== TopN ========
            PlanNodeEnum::TopN(topn_node) => {
                let input_plan = topn_node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let sort_items = topn_node.sort_items();
                let (sort_expressions, sort_directions) =
                    Self::sort_items_to_expressions(sort_items)?;
                Ok(StreamingExecutor::TopN {
                    input: Box::new(input_executor),
                    n: topn_node.limit() as u32,
                    sort_expressions,
                    sort_directions,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== RightJoin ========
            PlanNodeEnum::RightJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;
                Ok(StreamingExecutor::RightJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    build_side_tuples: vec![],
                    right_consumed: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== FullOuterJoin ========
            PlanNodeEnum::FullOuterJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;
                Ok(StreamingExecutor::FullOuterJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    left_rows: vec![],
                    right_rows: vec![],
                    matched_right_indices: std::collections::HashSet::new(),
                    result_iter: None,
                    phase: super::executor::FullOuterJoinPhase::BuildingRight,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== SemiJoin ========
            PlanNodeEnum::SemiJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;
                Ok(StreamingExecutor::SemiJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    right_rows: vec![],
                    right_consumed: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== HashInnerJoin (maps to HashJoin) ========
            PlanNodeEnum::HashInnerJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;
                let probe_keys = Self::join_keys_to_expressions(join_node.hash_keys())?;
                let hash_keys = Self::join_keys_to_expressions(join_node.probe_keys())?;
                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                Ok(StreamingExecutor::HashJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    hash_keys,
                    probe_keys,
                    build_side_hash: std::collections::HashMap::new(),
                    all_right_rows: vec![],
                    left_consumed: false,
                    opened: false,
                    memory_tracker,
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== HashLeftJoin (maps to HashLeftJoin) ========
            PlanNodeEnum::HashLeftJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let condition = Self::join_keys_to_condition(
                    join_node.hash_keys(),
                    join_node.probe_keys(),
                    right_plan.col_names(),
                )?;
                let probe_keys = Self::join_keys_to_expressions(join_node.hash_keys())?;
                let hash_keys = Self::join_keys_to_expressions(join_node.probe_keys())?;
                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                Ok(StreamingExecutor::HashLeftJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    hash_keys,
                    probe_keys,
                    build_side_hash: std::collections::HashMap::new(),
                    all_right_rows: vec![],
                    left_consumed: false,
                    opened: false,
                    memory_tracker,
                    right_col_names: vec![],
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== AppendVertices ========
            PlanNodeEnum::AppendVertices(node) => {
                let input_plan = node.inputs().first().ok_or_else(|| {
                    QueryError::execution("AppendVertices requires an input".to_string())
                })?;
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let vertex_properties: Vec<(String, Expression)> = node
                    .vertex_props()
                    .iter()
                    .flat_map(|p| {
                        let tag = p.tag.clone();
                        let props = p.props.clone();
                        props.into_iter().map(move |prop_name| {
                            (
                                prop_name.clone(),
                                Expression::Variable(format!("{}.{}", tag, prop_name)),
                            )
                        })
                    })
                    .collect();
                Ok(StreamingExecutor::AppendVertices {
                    input: Box::new(input_executor),
                    vertex_properties,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== BiExpand ========
            PlanNodeEnum::BiExpand(node) => {
                let left_plan = node.left_input();
                let right_plan = node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let _right_executor = Self::from_plan_node(right_plan, context)?;
                let edge_types = node.edge_types().to_vec();
                let direction = node.left_direction();
                Ok(StreamingExecutor::BiExpand {
                    input: Box::new(left_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    edge_types,
                    direction,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== BiTraverse ========
            PlanNodeEnum::BiTraverse(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let edge_types = node.edge_types().to_vec();
                let direction = node.left_direction();
                Ok(StreamingExecutor::BiTraverse {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    edge_types,
                    direction,
                    min_depth: node.min_hops() as u32,
                    max_depth: node.max_hops() as u32,
                    visited: std::collections::HashSet::new(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== ShortestPath ========
            PlanNodeEnum::ShortestPath(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let edge_types = node.edge_types().to_vec();
                let target = node.end_vertex_ids().first().cloned().map(Expression::Literal);
                Ok(StreamingExecutor::ShortestPath {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    target_vertex: target,
                    edge_types,
                    direction: crate::core::EdgeDirection::Both,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== BFSShortest ========
            PlanNodeEnum::BFSShortest(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let edge_types = node.edge_types().to_vec();
                let direction = if node.reverse() { crate::core::EdgeDirection::In } else { crate::core::EdgeDirection::Both };
                Ok(StreamingExecutor::BFSShortest {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    target_vertex: None,
                    edge_types,
                    direction,
                    frontier: vec![],
                    visited: std::collections::HashSet::new(),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== AllPaths ========
            PlanNodeEnum::AllPaths(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let edge_types = node.edge_types().to_vec();
                let target = node.end_vertex_ids().first().map(|id| {
                    Expression::Literal(Value::String(id.to_string()))
                });
                Ok(StreamingExecutor::AllPaths {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    target_vertex: target,
                    edge_types,
                    direction: crate::core::EdgeDirection::Both,
                    all_paths: vec![],
                    result_iter: None,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== MultiShortestPath ========
            PlanNodeEnum::MultiShortestPath(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::MultiShortestPath {
                    input: Box::new(input_executor),
                    storage: context.storage.clone(),
                    space_name: context.space_name.clone().unwrap_or_default(),
                    target_vertices: vec![],
                    edge_types: vec![],
                    direction: crate::core::EdgeDirection::Both,
                    all_paths: vec![],
                    result_iter: None,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== DataCollect ========
            PlanNodeEnum::DataCollect(node) => {
                let input_plan = node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::DataCollect {
                    input: Box::new(input_executor),
                    all_rows: vec![],
                    emitted: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Unwind ========
            PlanNodeEnum::Unwind(node) => {
                let input_plan = node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::Unwind {
                    input: Box::new(input_executor),
                    unwind_column: node.alias().to_string(),
                    col_index: None,
                    all_rows: vec![],
                    current_row_index: 0,
                    current_unwind_index: 0,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Materialize ========
            PlanNodeEnum::Materialize(node) => {
                let input_plan = node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                Ok(StreamingExecutor::Materialize {
                    input: Box::new(input_executor),
                    materialized_rows: vec![],
                    result_iter: None,
                    materialized: false,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Assign ========
            PlanNodeEnum::Assign(node) => {
                let input_plan = node.input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let assignments: Vec<(String, Expression)> = node
                    .assignments()
                    .iter()
                    .filter_map(|(name, expr)| expr.get_expression().map(|e| (name.clone(), e)))
                    .collect();
                Ok(StreamingExecutor::Assign {
                    input: Box::new(input_executor),
                    assignments,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Apply ========
            PlanNodeEnum::Apply(node) => {
                let left_plan = node.left_input();
                let right_plan = node.right_input();
                let left_executor = Self::from_plan_node(left_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let kind_label = match node.apply_kind() {
                    crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Standard => "standard",
                    crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Semi => "semi",
                    crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Anti => "anti",
                    crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Single => "single",
                    crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::All => "all",
                };
                Ok(StreamingExecutor::Apply {
                    input: Box::new(left_executor),
                    right: Box::new(right_executor),
                    apply_expression: Expression::Literal(Value::String(kind_label.to_string())),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== PatternApply ========
            PlanNodeEnum::PatternApply(node) => {
                let input_plan = node.left_input();
                let right_plan = node.right_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let right_executor = Self::from_plan_node(right_plan, context)?;
                let key_exprs: Vec<Expression> = node
                    .key_cols()
                    .iter()
                    .filter_map(|c| Self::contextual_to_expression(c).ok())
                    .collect();
                let pattern_expr = if key_exprs.is_empty() {
                    Expression::Literal(Value::Bool(true))
                } else if key_exprs.len() == 1 {
                    key_exprs.into_iter().next().unwrap()
                } else {
                    Expression::Literal(Value::String("correlated".to_string()))
                };
                Ok(StreamingExecutor::PatternApply {
                    input: Box::new(input_executor),
                    right: Box::new(right_executor),
                    pattern: pattern_expr,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== RollUpApply ========
            PlanNodeEnum::RollUpApply(node) => {
                let input_plan = node.left_input();
                let input_executor = Self::from_plan_node(input_plan, context)?;
                let rollup_expressions: Vec<Expression> = node
                    .compare_cols()
                    .iter()
                    .map(|c| Expression::Variable(c.clone()))
                    .collect();
                Ok(StreamingExecutor::RollUpApply {
                    input: Box::new(input_executor),
                    rollup_expressions,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== Loop ========
            PlanNodeEnum::Loop(node) => {
                let body_executor = if let Some(body) = node.body() {
                    Self::from_plan_node(body, context)?
                } else {
                    StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }
                };
                Ok(StreamingExecutor::Loop {
                    input: Box::new(body_executor),
                    condition: Some(format!("{:?}", node.condition())),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== PassThrough ========
            PlanNodeEnum::PassThrough(_) => Ok(StreamingExecutor::PassThrough {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== Select ========
            PlanNodeEnum::Select(node) => {
                let branch_executor = if let Some(if_branch) = node.if_branch() {
                    Self::from_plan_node(if_branch, context)?
                } else if let Some(else_branch) = node.else_branch() {
                    Self::from_plan_node(else_branch, context)?
                } else {
                    StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }
                };
                Ok(StreamingExecutor::Select {
                    input: Box::new(branch_executor),
                    selection_expr: None,
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }

            // ======== BeginTransaction ========
            PlanNodeEnum::BeginTransaction(_) => Ok(StreamingExecutor::BeginTransaction {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                transaction_id: None,
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== ShowStats ========
            PlanNodeEnum::ShowStats(_) => Ok(StreamingExecutor::ShowStats {
                input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                opened: false,
                plan_node_id: node.id(),
                runtime: None,
            }),

            // ======== DeleteIndex (map to IndexManage for backward compat) ========
            PlanNodeEnum::DeleteIndex(node) => {
                let info = node.info();
                Ok(StreamingExecutor::IndexManage {
                    input: Box::new(StreamingExecutor::Start { opened: false, plan_node_id: 0, runtime: None }),
                    storage: context.storage.clone(),
                    space_name: info.space_name.clone(),
                    action: "drop_tag_index".to_string(),
                    index_name: Some(info.index_name.clone()),
                    opened: false,
                    plan_node_id: node.id(),
                    runtime: None,
                })
            }
        }
    }

    /// Helper: extract space name from SpaceManageNode
    fn extract_space_manage_name(node: &SpaceManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode::*;
        match node {
            Create(n) => Some(n.info().space_name.clone()),
            Drop(n) => Some(n.space_name().to_string()),
            Desc(n) => Some(n.space_name().to_string()),
            Show(_) => None,
            ShowCreate(n) => Some(n.space_name().to_string()),
            Switch(n) => Some(n.space_name().to_string()),
            Alter(n) => Some(n.space_name().to_string()),
            Clear(n) => Some(n.space_name().to_string()),
        }
    }

    /// Helper: extract tag name from TagManageNode
    fn extract_tag_manage_name(node: &TagManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode::*;
        match node {
            Create(n) => Some(n.info().tag_name.clone()),
            Alter(n) => Some(n.info().tag_name.clone()),
            Desc(n) => Some(n.tag_name().to_string()),
            Drop(n) => Some(n.tag_name().to_string()),
            Show(_) => None,
            ShowCreate(n) => Some(n.tag_name().to_string()),
        }
    }

    /// Helper: extract properties from TagManageNode (only for Create variant)
    fn extract_tag_manage_properties(node: &TagManageNode) -> Vec<crate::core::types::PropertyDef> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode::*;
        match node {
            Create(n) => n.info().properties.clone(),
            _ => Vec::new(),
        }
    }

    /// Helper: extract properties from EdgeManageNode (only for Create variant)
    fn extract_edge_manage_properties(
        node: &EdgeManageNode,
    ) -> Vec<crate::core::types::PropertyDef> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode::*;
        match node {
            Create(n) => n.info().properties.clone(),
            _ => Vec::new(),
        }
    }

    /// Helper: extract edge type name from EdgeManageNode
    fn extract_edge_manage_name(node: &EdgeManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode::*;
        match node {
            Create(n) => Some(n.info().edge_name.clone()),
            Alter(n) => Some(n.info().edge_name.clone()),
            Desc(n) => Some(n.edge_name().to_string()),
            Drop(n) => Some(n.edge_name().to_string()),
            Show(_) => None,
            ShowCreate(n) => Some(n.edge_name().to_string()),
        }
    }

    /// Helper: extract index name from IndexManageNode
    fn extract_index_manage_name(node: &IndexManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode::*;
        match node {
            CreateTagIndex(n) => Some(n.info().index_name.clone()),
            DropTagIndex(n) => Some(n.index_name().to_string()),
            DescTagIndex(n) => Some(n.index_name().to_string()),
            ShowTagIndexes(_) => None,
            RebuildTagIndex(n) => Some(n.index_name().to_string()),
            CreateEdgeIndex(n) => Some(n.info().index_name.clone()),
            DropEdgeIndex(n) => Some(n.index_name().to_string()),
            DescEdgeIndex(n) => Some(n.index_name().to_string()),
            ShowEdgeIndexes(_) => None,
            RebuildEdgeIndex(n) => Some(n.index_name().to_string()),
            ShowIndexes(_) => None,
            ShowCreateIndex(n) => Some(n.index_name().to_string()),
        }
    }

    /// Helper: extract username from UserManageNode
    fn extract_user_manage_name(node: &UserManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::*;
        match node {
            Create(n) => Some(n.username().to_string()),
            Alter(n) => Some(n.username().to_string()),
            Drop(n) => Some(n.username().to_string()),
            ChangePassword(_) => None,
            GrantRole(n) => Some(n.username().to_string()),
            RevokeRole(n) => Some(n.username().to_string()),
            DescribeUser(n) => Some(n.username().to_string()),
            ShowRoles(_) => None,
            ShowUsers(_) => None,
        }
    }

    /// Helper: extract index name from FulltextManageNode
    fn extract_fulltext_manage_name(node: &FulltextManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode::*;
        match node {
            Create(n) => Some(n.index_name.clone()),
            Alter(n) => Some(n.index_name.clone()),
            Describe(n) => Some(n.index_name.clone()),
            Drop(n) => Some(n.index_name.clone()),
            Show(_) => None,
        }
    }

    /// Helper: extract index name from VectorManageNode
    fn extract_vector_manage_name(node: &VectorManageNode) -> Option<String> {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode::*;
        match node {
            Create(n) => Some(n.index_name.clone()),
            Drop(n) => Some(n.index_name.clone()),
        }
    }

    /// Convert FulltextQueryExpr to a simple query string
    fn fulltext_query_to_string(expr: &FulltextQueryExpr) -> String {
        match expr {
            FulltextQueryExpr::Simple(text) => text.clone(),
            FulltextQueryExpr::Field(field, text) => format!("{}:{}", field, text),
            FulltextQueryExpr::Phrase(text) => format!("\"{}\"", text),
            FulltextQueryExpr::Prefix(text) => format!("{}*", text),
            FulltextQueryExpr::Fuzzy(text, distance) => {
                if let Some(d) = distance {
                    format!("{}~{}", text, d)
                } else {
                    format!("{}~", text)
                }
            }
            FulltextQueryExpr::Wildcard(text) => text.clone(),
            FulltextQueryExpr::Boolean {
                must,
                should,
                must_not,
            } => {
                let mut parts = Vec::new();
                for e in must {
                    parts.push(format!("+({})", Self::fulltext_query_to_string(e)));
                }
                for e in should {
                    parts.push(format!("({})", Self::fulltext_query_to_string(e)));
                }
                for e in must_not {
                    parts.push(format!("-({})", Self::fulltext_query_to_string(e)));
                }
                parts.join(" ")
            }
            FulltextQueryExpr::MultiField(fields) => fields
                .iter()
                .map(|(f, t)| format!("{}:{}", f, t))
                .collect::<Vec<_>>()
                .join(" OR "),
            FulltextQueryExpr::Range {
                field,
                lower,
                upper,
                include_lower,
                include_upper,
            } => {
                let lower_bound = if *include_lower { "[" } else { "{" };
                let upper_bound = if *include_upper { "]" } else { "}" };
                let lower_val = lower.as_deref().unwrap_or("*");
                let upper_val = upper.as_deref().unwrap_or("*");
                format!(
                    "{}:{}{} TO {}{}",
                    field, lower_bound, lower_val, upper_val, upper_bound
                )
            }
        }
    }

    /// Convert FulltextMatchCondition to a simple string
    fn fulltext_match_to_string(
        condition: &crate::query::parser::ast::fulltext::FulltextMatchCondition,
    ) -> String {
        format!("{}:{}", condition.field, condition.query)
    }

    /// Convert VectorQueryExpr to Vec<f32>
    #[cfg(feature = "qdrant")]
    fn vector_query_to_vec(expr: &crate::query::parser::ast::vector::VectorQueryExpr) -> Vec<f32> {
        serde_json::from_str(&expr.query_data).unwrap_or_default()
    }

    /// Recursively convert a plan DAG to a streaming executor
    pub fn from_plan(
        plan: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        Self::from_plan_node(plan, context)
    }

    /// Helper: Convert ContextualExpression to Expression
    fn contextual_to_expression(
        expr: &crate::core::types::expr::ContextualExpression,
    ) -> Result<Expression, QueryError> {
        // Extract the underlying expression from ContextualExpression
        expr.get_expression().ok_or_else(|| {
            QueryError::execution("Failed to get expression from ContextualExpression".to_string())
        })
    }

    fn contextual_to_value(
        expr: &crate::core::types::expr::ContextualExpression,
    ) -> Result<Value, QueryError> {
        if let Some(value) = expr.constant_value() {
            return Ok(value);
        }

        match Self::contextual_to_expression(expr)? {
            Expression::Literal(value) => Ok(value),
            other => Err(QueryError::execution(format!(
                "Standalone data modification requires constant values, got {:?}",
                other
            ))),
        }
    }

    fn join_keys_to_condition(
        hash_keys: &[crate::core::types::expr::ContextualExpression],
        probe_keys: &[crate::core::types::expr::ContextualExpression],
        right_col_names: &[String],
    ) -> Result<Option<Expression>, QueryError> {
        if hash_keys.is_empty() && probe_keys.is_empty() {
            return Ok(None);
        }
        if hash_keys.len() != probe_keys.len() {
            return Err(QueryError::execution(format!(
                "Join key count mismatch: {} hash keys vs {} probe keys",
                hash_keys.len(),
                probe_keys.len()
            )));
        }

        let mut condition = None;
        for (hash_key, probe_key) in hash_keys.iter().zip(probe_keys.iter()) {
            let left = Self::contextual_to_expression(hash_key)?;
            let right = Self::rewrite_right_join_expr(
                Self::contextual_to_expression(probe_key)?,
                right_col_names,
            );
            let equality = Expression::Binary {
                left: Box::new(left),
                op: BinaryOperator::Equal,
                right: Box::new(right),
            };
            condition = Some(match condition {
                Some(existing) => Expression::Binary {
                    left: Box::new(existing),
                    op: BinaryOperator::And,
                    right: Box::new(equality),
                },
                None => equality,
            });
        }

        Ok(condition)
    }

    fn join_keys_to_expressions(
        keys: &[crate::core::types::expr::ContextualExpression],
    ) -> Result<Vec<Expression>, QueryError> {
        keys.iter().map(Self::contextual_to_expression).collect()
    }

    fn rewrite_right_join_expr(expr: Expression, right_col_names: &[String]) -> Expression {
        match expr {
            Expression::Variable(name) => {
                if let Some(index) = right_col_names.iter().position(|col| col == &name) {
                    Expression::Variable(format!("right_{}", index))
                } else {
                    Expression::Variable(name)
                }
            }
            Expression::Binary { left, op, right } => Expression::Binary {
                left: Box::new(Self::rewrite_right_join_expr(*left, right_col_names)),
                op,
                right: Box::new(Self::rewrite_right_join_expr(*right, right_col_names)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op,
                operand: Box::new(Self::rewrite_right_join_expr(*operand, right_col_names)),
            },
            other => other,
        }
    }

    /// Helper: Convert YieldColumn to Expressions
    fn yield_columns_to_expressions(
        columns: &[crate::core::YieldColumn],
    ) -> Result<Vec<Expression>, QueryError> {
        columns
            .iter()
            .map(|col| {
                // YieldColumn has an expression field of type ContextualExpression
                Self::contextual_to_expression(&col.expression)
            })
            .collect()
    }

    /// Helper: Convert SortItem to (Expression, SortDirection) pairs
    fn sort_items_to_expressions(
        items: &[crate::query::planning::plan::core::nodes::operation::sort_node::SortItem],
    ) -> Result<(Vec<Expression>, Vec<super::executor::SortDirection>), QueryError> {
        let mut expressions = Vec::new();
        let mut directions = Vec::new();

        for item in items {
            expressions.push(item.expression.clone());
            let direction = match item.direction {
                crate::core::types::graph_schema::OrderDirection::Asc => {
                    super::executor::SortDirection::Ascending
                }
                crate::core::types::graph_schema::OrderDirection::Desc => {
                    super::executor::SortDirection::Descending
                }
            };
            directions.push(direction);
        }

        Ok((expressions, directions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_scan_creation() {
        let rows = vec![
            vec![Value::BigInt(1), Value::String("a".to_string())],
            vec![Value::BigInt(2), Value::String("b".to_string())],
        ];
        let executor = StreamingExecutorBuilder::build_simple_scan(rows.clone()).unwrap();
        match executor {
            StreamingExecutor::ScanVertices { buffer, .. } => {
                assert_eq!(buffer.len(), 2);
            }
            _ => panic!("Expected ScanVertices"),
        }
    }

    #[test]
    fn test_empty_scan_creation() {
        let rows: Vec<Vec<Value>> = vec![];
        let executor = StreamingExecutorBuilder::build_simple_scan(rows).unwrap();
        match executor {
            StreamingExecutor::ScanVertices { buffer, .. } => {
                assert!(buffer.is_empty());
            }
            _ => panic!("Expected ScanVertices"),
        }
    }
}
