//! Builder for StreamingExecutor from ExecutionPlan
//!
//! This module provides utilities to construct streaming executors
//! from the execution plan. Supports automatic conversion of plan nodes
//! to streaming executor operators.

use super::executor::StreamingExecutor;
use super::partition::PartitionView;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{MultipleInputNode, SingleInputNode};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::{
    EdgeManageNode, FulltextManageNode, IndexManageNode, SpaceManageNode, TagManageNode,
    UserManageNode, VectorManageNode,
};


/// Builder for constructing StreamingExecutor instances
pub struct StreamingExecutorBuilder {
    partition_view: PartitionView,
}

impl StreamingExecutorBuilder {
    /// Create a new builder with partition view
    pub fn new(partition_view: PartitionView) -> Self {
        Self { partition_view }
    }

    /// Create a simple scan executor for testing/basic use
    pub fn build_simple_scan(rows: Vec<Vec<Value>>) -> Result<StreamingExecutor, QueryError> {
        if rows.is_empty() {
            return Ok(StreamingExecutor::ScanVertices {
                partition_id: 0,
                buffer: vec![],
                current_index: 0,
            });
        }

        Ok(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: rows,
            current_index: 0,
        })
    }

    /// Convert a single plan node to a streaming executor
    /// Supports: ScanVertices, ScanEdges, Filter, Project, Limit, Aggregate, and Join operations
    pub fn from_plan_node(
        node: &PlanNodeEnum,
        _context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        match node {
            // ======== Scan Operations ========
            PlanNodeEnum::ScanVertices(scan_node) => {
                let limit = scan_node.limit();
                let executor = StreamingExecutor::ScanVertices {
                    partition_id: 0,
                    buffer: vec![],
                    current_index: 0,
                };

                // If limit is specified, wrap in Limit operator
                if let Some(limit_val) = limit {
                    Ok(StreamingExecutor::Limit {
                        input: Box::new(executor),
                        limit: limit_val as u32,
                        consumed: 0,
                        opened: false,
                    })
                } else {
                    Ok(executor)
                }
            }

            PlanNodeEnum::ScanEdges(scan_node) => {
                let limit = scan_node.limit();
                let executor = StreamingExecutor::ScanEdges {
                    partition_id: 0,
                    buffer: vec![],
                    current_index: 0,
                };

                // If limit is specified, wrap in Limit operator
                if let Some(limit_val) = limit {
                    Ok(StreamingExecutor::Limit {
                        input: Box::new(executor),
                        limit: limit_val as u32,
                        consumed: 0,
                        opened: false,
                    })
                } else {
                    Ok(executor)
                }
            }

            // ======== Filter Operation ========
            PlanNodeEnum::Filter(filter_node) => {
                let input_plan = filter_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let condition = filter_node.condition();
                let predicate = Self::contextual_to_expression(condition)?;

                Ok(StreamingExecutor::Filter {
                    input: Box::new(input_executor),
                    predicate,
                    opened: false,
                })
            }

            // ======== Project Operation ========
            PlanNodeEnum::Project(project_node) => {
                let input_plan = project_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let columns = project_node.columns();
                let output_expressions = Self::yield_columns_to_expressions(columns)?;

                Ok(StreamingExecutor::Project {
                    input: Box::new(input_executor),
                    output_expressions,
                    opened: false,
                })
            }

            // ======== Limit Operation ========
            PlanNodeEnum::Limit(limit_node) => {
                let input_plan = limit_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

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
                })
            }

            // ======== Aggregate Operation ========
            PlanNodeEnum::Aggregate(agg_node) => {
                let input_plan = agg_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

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
                            AggregateFunction::Count(Some(field)) => Expression::Variable(field.clone()),
                            AggregateFunction::Sum(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Avg(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Min(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Max(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Collect(field) => Expression::Variable(field.clone()),
                            AggregateFunction::Count(None) => Expression::Literal(Value::Int(1)),
                            _ => Expression::Literal(Value::Int(1)),
                        };
                        (func.clone(), expr)
                    })
                    .collect();

                Ok(StreamingExecutor::Aggregate {
                    input: Box::new(input_executor),
                    group_by_expressions,
                    aggregate_functions,
                    all_rows: vec![],
                    result_iter: None,
                    opened: false,
                })
            }

            // ======== Join Operations ========
            PlanNodeEnum::InnerJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                // For joins, we use hash_keys and probe_keys instead of explicit conditions
                // Create a simple condition from the first pair of keys if available
                let condition = None; // Simplified: join conditions will be handled via hash/probe keys

                Ok(StreamingExecutor::HashJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: condition,
                    build_side_tuples: vec![],
                    left_consumed: false,
                    opened: false,
                })
            }

            PlanNodeEnum::LeftJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                Ok(StreamingExecutor::HashJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: None,
                    build_side_tuples: vec![],
                    left_consumed: false,
                    opened: false,
                })
            }

            PlanNodeEnum::CrossJoin(join_node) => {
                let left_plan = join_node.left_input();
                let right_plan = join_node.right_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                // Cross join has no condition
                Ok(StreamingExecutor::NestedLoopJoin {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    join_condition: None,
                    build_side_tuples: vec![],
                    left_consumed: false,
                    opened: false,
                })
            }

            // ======== Set Operations ========
            PlanNodeEnum::Union(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.union_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                Ok(StreamingExecutor::Union {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    seen_rows: std::collections::HashSet::new(),
                    left_consumed: false,
                    opened: false,
                })
            }

            PlanNodeEnum::Intersect(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.intersect_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                Ok(StreamingExecutor::Intersect {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    left_rows: std::collections::HashSet::new(),
                    right_rows: std::collections::HashSet::new(),
                    left_buffered: false,
                    right_buffered: false,
                    opened: false,
                })
            }

            PlanNodeEnum::Minus(set_node) => {
                let left_plan = set_node.input();
                let right_plan = set_node.minus_input();
                let left_executor = Self::from_plan_node(left_plan, _context)?;
                let right_executor = Self::from_plan_node(right_plan, _context)?;

                Ok(StreamingExecutor::Except {
                    left: Box::new(left_executor),
                    right: Box::new(right_executor),
                    exclude_rows: std::collections::HashSet::new(),
                    right_buffered: false,
                    opened: false,
                })
            }

            // ======== Other Operations (partially supported) ========
            PlanNodeEnum::Sort(sort_node) => {
                let input_plan = sort_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let sort_items = sort_node.sort_items();
                if sort_items.is_empty() {
                    // No sort items: pass through
                    Ok(input_executor)
                } else {
                    let (sort_expressions, sort_directions) =
                        Self::sort_items_to_expressions(sort_items)?;

                    Ok(StreamingExecutor::Sort {
                        input: Box::new(input_executor),
                        sort_expressions,
                        sort_directions,
                        all_rows: vec![],
                        row_iter: None,
                        opened: false,
                    })
                }
            }

            PlanNodeEnum::Dedup(dedup_node) => {
                let input_plan = dedup_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                Ok(StreamingExecutor::Distinct {
                    input: Box::new(input_executor),
                    seen_rows: std::collections::HashSet::new(),
                    opened: false,
                })
            }

            // ======== Management/DDL Operations ========
            PlanNodeEnum::SpaceManage(manage_node) => {
                let action = manage_node.name().to_string();
                let space_name = Self::extract_space_manage_name(manage_node);
                Ok(StreamingExecutor::SpaceManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    space_name,
                    opened: false,
                })
            }

            PlanNodeEnum::TagManage(manage_node) => {
                let action = manage_node.name().to_string();
                let tag_name = Self::extract_tag_manage_name(manage_node);
                Ok(StreamingExecutor::TagManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    tag_name,
                    opened: false,
                })
            }

            PlanNodeEnum::EdgeManage(manage_node) => {
                let action = manage_node.name().to_string();
                let edge_type = Self::extract_edge_manage_name(manage_node);
                Ok(StreamingExecutor::EdgeManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    edge_type,
                    opened: false,
                })
            }

            PlanNodeEnum::IndexManage(manage_node) => {
                let action = manage_node.name().to_string();
                let index_name = Self::extract_index_manage_name(manage_node);
                Ok(StreamingExecutor::IndexManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    index_name,
                    opened: false,
                })
            }

            PlanNodeEnum::UserManage(manage_node) => {
                let action = manage_node.name().to_string();
                let username = Self::extract_user_manage_name(manage_node);
                Ok(StreamingExecutor::UserManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    username,
                    opened: false,
                })
            }

            PlanNodeEnum::FulltextManage(manage_node) => {
                let action = manage_node.name().to_string();
                let index_name = Self::extract_fulltext_manage_name(manage_node);
                Ok(StreamingExecutor::FulltextManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    index_name,
                    opened: false,
                })
            }

            PlanNodeEnum::VectorManage(manage_node) => {
                let action = manage_node.name().to_string();
                let index_name = Self::extract_vector_manage_name(manage_node);
                Ok(StreamingExecutor::VectorManage {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    action,
                    index_name,
                    opened: false,
                })
            }

            // ======== Graph Traversal Operations ========
            PlanNodeEnum::Expand(expand_node) => {
                let input_plan = expand_node.inputs().first()
                    .ok_or_else(|| QueryError::execution("Expand requires an input".to_string()))?;
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let edge_type = expand_node.edge_types().first()
                    .cloned()
                    .unwrap_or_default();
                let direction = format!("{:?}", expand_node.direction());

                Ok(StreamingExecutor::Expand {
                    input: Box::new(input_executor),
                    edge_type,
                    direction,
                    filter_expr: None,
                    opened: false,
                })
            }

            PlanNodeEnum::ExpandAll(expand_all_node) => {
                let input_plan = expand_all_node.inputs().first()
                    .ok_or_else(|| QueryError::execution("ExpandAll requires an input".to_string()))?;
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let edge_type = expand_all_node.edge_types().first()
                    .cloned()
                    .unwrap_or_default();
                let direction = expand_all_node.direction().to_string();

                Ok(StreamingExecutor::ExpandAll {
                    input: Box::new(input_executor),
                    edge_type,
                    direction,
                    filter_expr: None,
                    opened: false,
                })
            }

            PlanNodeEnum::Traverse(traverse_node) => {
                let input_plan = traverse_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;

                let edge_type = traverse_node.edge_types().first()
                    .cloned()
                    .unwrap_or_default();
                let direction = format!("{:?}", traverse_node.direction());
                let min_depth = traverse_node.min_steps();
                let max_depth = traverse_node.max_steps();

                Ok(StreamingExecutor::Traverse {
                    input: Box::new(input_executor),
                    edge_type,
                    direction,
                    min_depth,
                    max_depth,
                    filter_expr: None,
                    visited: std::collections::HashSet::new(),
                    opened: false,
                })
            }

            // ======== Data Modification Operations ========
            PlanNodeEnum::InsertVertices(insert_node) => {
                Ok(StreamingExecutor::InsertVertices {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    vertex_properties: Vec::new(),
                    tags: insert_node.tag_names(),
                    rows_inserted: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::InsertEdges(insert_node) => {
                Ok(StreamingExecutor::InsertEdges {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    edge_type: insert_node.edge_name().to_string(),
                    edge_properties: Vec::new(),
                    rows_inserted: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::UpdateVertices(_update_node) => {
                Ok(StreamingExecutor::UpdateVertices {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    updates: Vec::new(),
                    rows_updated: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::Update(_update_node) => {
                Ok(StreamingExecutor::UpdateVertices {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    updates: Vec::new(),
                    rows_updated: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::UpdateEdges(_update_node) => {
                Ok(StreamingExecutor::UpdateEdges {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    updates: Vec::new(),
                    rows_updated: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::DeleteVertices(_delete_node) => {
                Ok(StreamingExecutor::DeleteVertices {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    vertex_id_col: "vid".to_string(),
                    rows_deleted: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::DeleteEdges(_delete_node) => {
                Ok(StreamingExecutor::DeleteEdges {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    rows_deleted: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::PipeDeleteVertices(delete_node) => {
                let input_plan = delete_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;
                Ok(StreamingExecutor::PipeDeleteVertices {
                    input: Box::new(input_executor),
                    vertex_id_col: "vid".to_string(),
                    rows_deleted: 0,
                    opened: false,
                })
            }

            PlanNodeEnum::PipeDeleteEdges(delete_node) => {
                let input_plan = delete_node.input();
                let input_executor = Self::from_plan_node(input_plan, _context)?;
                Ok(StreamingExecutor::PipeDeleteEdges {
                    input: Box::new(input_executor),
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    rows_deleted: 0,
                    opened: false,
                })
            }

            // ======== Data Access Operations ========
            PlanNodeEnum::GetVertices(_get_node) => {
                Ok(StreamingExecutor::GetVertices {
                    opened: false,
                })
            }

            PlanNodeEnum::GetEdges(_get_node) => {
                Ok(StreamingExecutor::GetEdges {
                    opened: false,
                })
            }

            PlanNodeEnum::GetNeighbors(_get_node) => {
                Ok(StreamingExecutor::GetNeighbors {
                    opened: false,
                })
            }

            // ======== Search Operations ========
            PlanNodeEnum::FulltextSearch(search_node) => {
                Ok(StreamingExecutor::FulltextSearch {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    search_query: search_node.index_name.clone(),
                    search_field: Some(search_node.field_name.clone()),
                    opened: false,
                })
            }

            PlanNodeEnum::BeginTransaction(_) => {
                Ok(StreamingExecutor::BeginTransaction {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    transaction_id: None,
                    opened: false,
                })
            }

            PlanNodeEnum::Commit(_) => {
                Ok(StreamingExecutor::Commit {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    transaction_id: None,
                    opened: false,
                })
            }

            PlanNodeEnum::Rollback(_) => {
                Ok(StreamingExecutor::Rollback {
                    input: Box::new(StreamingExecutor::Start { opened: false }),
                    transaction_id: None,
                    opened: false,
                })
            }

            _ => {
                // Unsupported node types for streaming execution
                Err(QueryError::execution(format!(
                    "Plan node type '{}' not yet supported in streaming executor",
                    node.name()
                )))
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
        expr.get_expression()
            .ok_or_else(|| QueryError::execution("Failed to get expression from ContextualExpression".to_string()))
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

    /// Get partition view reference
    pub fn partition_view(&self) -> &PartitionView {
        &self.partition_view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_creation() {
        let pv = PartitionView::single(0..1000);
        let builder = StreamingExecutorBuilder::new(pv);
        assert_eq!(builder.partition_view().partition_count, 1);
    }

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
