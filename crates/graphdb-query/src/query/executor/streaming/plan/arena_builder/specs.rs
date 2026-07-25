//! Translation from logical planner nodes to executable operator specifications.

use std::sync::Arc;

use super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, FulltextSpec, GraphSpec, JoinSpec, RecursiveFragmentSpec,
    SinkSpec, SourceSpec, UnarySpec, VectorSpec,
};
use super::super::super::slot::SlotLayout;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;

fn fulltext_query_to_string(
    expr: &crate::query::parser::ast::fulltext::FulltextQueryExpr,
) -> String {
    match expr {
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Simple(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Phrase(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Prefix(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Wildcard(text) => text.clone(),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Field(field, text) => {
            format!("{field}:{text}")
        }
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Fuzzy(text, distance) => distance
            .map_or_else(
                || format!("{text}~"),
                |distance| format!("{text}~{distance}"),
            ),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Boolean {
            must,
            should,
            must_not,
        } => must
            .iter()
            .map(|item| format!("+({})", fulltext_query_to_string(item)))
            .chain(
                should
                    .iter()
                    .map(|item| format!("({})", fulltext_query_to_string(item))),
            )
            .chain(
                must_not
                    .iter()
                    .map(|item| format!("-({})", fulltext_query_to_string(item))),
            )
            .collect::<Vec<_>>()
            .join(" "),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::MultiField(fields) => fields
            .iter()
            .map(|(field, text)| format!("{field}:{text}"))
            .collect::<Vec<_>>()
            .join(" OR "),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Range {
            field,
            lower,
            upper,
            include_lower,
            include_upper,
        } => format!(
            "{field}:{}{} TO {}{}",
            if *include_lower { "[" } else { "{" },
            lower.as_deref().unwrap_or("*"),
            upper.as_deref().unwrap_or("*"),
            if *include_upper { "]" } else { "}" },
        ),
    }
}

pub(super) fn build_source_spec(
    node: &PlanNodeEnum,
    exec_ctx: &ExecutionContext,
) -> Result<SourceSpec, PlanBuildError> {
    match node {
        PlanNodeEnum::Start(_) => Ok(SourceSpec::Start),
        PlanNodeEnum::Argument(_) => Ok(SourceSpec::Argument),
        PlanNodeEnum::ScanVertices(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|v| (v >= 0).then_some(v as usize));
            Ok(SourceSpec::StorageScanVertices {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                limit,
                col_names: scan_node.col_names().to_vec(),
                projected_properties: scan_node.projected_properties().to_vec(),
            })
        }
        PlanNodeEnum::ScanEdges(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|v| (v >= 0).then_some(v as usize));
            Ok(SourceSpec::StorageScanEdges {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                limit,
                edge_type: scan_node.edge_type().map(|s| s.to_string()),
                col_names: scan_node.col_names().to_vec(),
                projected_properties: scan_node.projected_properties().to_vec(),
            })
        }
        PlanNodeEnum::GetVertices(get_node) => {
            let vertex_ids = get_node
                .src_ref()
                .constant_value()
                .map(|v| vec![v])
                .or_else(|| {
                    let src_vids = get_node.src_vids();
                    if src_vids.is_empty() {
                        None
                    } else {
                        Some(
                            src_vids
                                .split(',')
                                .map(|s| crate::core::Value::String(s.trim().to_string()))
                                .collect::<Vec<_>>(),
                        )
                    }
                });
            Ok(SourceSpec::GetVertices {
                space_name: get_node.space_name().to_string(),
                vertex_ids,
                projected_properties: get_node.projected_properties().to_vec(),
            })
        }
        PlanNodeEnum::GetEdges(get_node) => Ok(SourceSpec::GetEdges {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            edge_type: Some(get_node.edge_type().to_string()),
            src: Some(get_node.src().to_string()),
            dst: Some(get_node.dst().to_string()),
            rank: 0,
        }),
        PlanNodeEnum::GetNeighbors(get_node) => Ok(SourceSpec::GetNeighbors {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            direction: get_node.direction().to_string(),
            projected_properties: get_node.projected_properties().to_vec(),
        }),
        PlanNodeEnum::EdgeIndexScan(scan_node) => Ok(SourceSpec::EdgeIndexScan {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            edge_type: Some(scan_node.edge_type().to_string()),
        }),
        PlanNodeEnum::IndexScan(scan_node) => {
            let index_name = scan_node.index_name().to_string();
            if scan_node.filter().is_some() && scan_node.scan_limits().is_empty() {
                return Err(PlanBuildError::capability(
                    "index_residual_filter",
                    format!(
                        "IndexScan '{}' has a residual filter not pushed into native index cursor",
                        index_name
                    ),
                ));
            }
            let predicate = scan_node.scan_limits().first().map(|limit| {
                let column = limit.column.clone();
                match limit.scan_type {
                    crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Unique => {
                        crate::query::executor::streaming::operators::spec::BoundIndexPredicate::Equal {
                            column,
                            value: limit.begin_value.clone().unwrap_or(crate::core::Value::Null(crate::core::value::NullType::Null)),
                        }
                    }
                    crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Range => {
                        crate::query::executor::streaming::operators::spec::BoundIndexPredicate::Range {
                            column,
                            begin: limit.begin_value.clone(),
                            end: limit.end_value.clone(),
                            include_begin: limit.include_begin,
                            include_end: limit.include_end,
                        }
                    }
                    crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Prefix => {
                        crate::query::executor::streaming::operators::spec::BoundIndexPredicate::Prefix {
                            column,
                            prefix: limit.begin_value.clone().unwrap_or(crate::core::Value::Null(crate::core::value::NullType::Null)),
                        }
                    }
                    crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Full => {
                        crate::query::executor::streaming::operators::spec::BoundIndexPredicate::Full
                    }
                }
            }).unwrap_or(crate::query::executor::streaming::operators::spec::BoundIndexPredicate::Full);
            let projection = if scan_node.return_columns().is_empty() {
                crate::query::executor::streaming::operators::spec::IndexProjection::RowIdOnly
            } else {
                crate::query::executor::streaming::operators::spec::IndexProjection::Columns(
                    scan_node.return_columns().to_vec(),
                )
            };
            let col_names = scan_node.col_names().to_vec();
            let output_layout = Arc::new(SlotLayout::from_names(&col_names));
            Ok(SourceSpec::IndexScan {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                index_name,
                index_id: scan_node.index_id(),
                predicate,
                projection,
                residual_filter: None,
                output_layout,
            })
        }
        _ => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            "not a source node",
        )),
    }
}

fn contextual_to_value(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<crate::core::Value, PlanBuildError> {
    if let Some(value) = expr.constant_value() {
        return Ok(value);
    }
    if let Some(value) = expr.as_literal() {
        return Ok(value);
    }
    Err(PlanBuildError::expression(
        "DataModification",
        0,
        format!("{expr:?}"),
        "Standalone data modification requires constant values",
    ))
}

pub(super) fn build_standalone_write_source(
    node: &PlanNodeEnum,
) -> Result<SourceSpec, PlanBuildError> {
    let (rows, col_names) = match node {
        PlanNodeEnum::InsertVertices(insert) => {
            let property_names = insert
                .tags()
                .iter()
                .flat_map(|tag| tag.prop_names.iter().cloned())
                .collect::<Vec<_>>();
            let mut rows = Vec::with_capacity(insert.values().len());
            for (vertex_id, tag_values) in insert.values() {
                let mut row = vec![contextual_to_value(vertex_id)?];
                for values in tag_values {
                    for value in values {
                        row.push(contextual_to_value(value)?);
                    }
                }
                rows.push(row);
            }
            let mut names = vec!["vid".to_string()];
            names.extend(property_names);
            (rows, names)
        }
        PlanNodeEnum::InsertEdges(insert) => {
            let mut rows = Vec::with_capacity(insert.edges().len());
            for (src, dst, rank, properties) in insert.edges() {
                let mut row = vec![contextual_to_value(src)?, contextual_to_value(dst)?];
                row.push(match rank {
                    Some(value) => contextual_to_value(value)?,
                    None => crate::core::Value::BigInt(0),
                });
                for property in properties {
                    row.push(contextual_to_value(property)?);
                }
                rows.push(row);
            }
            let mut names = vec!["src".to_string(), "dst".to_string(), "rank".to_string()];
            names.extend(insert.prop_names().iter().cloned());
            (rows, names)
        }
        PlanNodeEnum::DeleteVertices(delete) => (
            delete
                .vertex_ids()
                .iter()
                .map(|value| contextual_to_value(value).map(|value| vec![value]))
                .collect::<Result<Vec<_>, _>>()?,
            vec!["vid".to_string()],
        ),
        PlanNodeEnum::DeleteEdges(delete) => (
            delete
                .edges()
                .iter()
                .map(|(src, dst, _)| Ok(vec![contextual_to_value(src)?, contextual_to_value(dst)?]))
                .collect::<Result<Vec<_>, PlanBuildError>>()?,
            vec!["src".to_string(), "dst".to_string()],
        ),
        PlanNodeEnum::Update(update) => {
            use crate::query::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
            match update.info() {
                UpdateTargetType::Vertex(info) => (
                    vec![vec![contextual_to_value(&info.vertex_id)?]],
                    vec!["vid".to_string()],
                ),
                UpdateTargetType::Edge(info) => (
                    vec![vec![
                        contextual_to_value(&info.src)?,
                        contextual_to_value(&info.dst)?,
                    ]],
                    vec!["src".to_string(), "dst".to_string()],
                ),
            }
        }
        PlanNodeEnum::UpdateVertices(update) => (
            update
                .updates()
                .iter()
                .map(|value| contextual_to_value(&value.vertex_id).map(|value| vec![value]))
                .collect::<Result<Vec<_>, _>>()?,
            vec!["vid".to_string()],
        ),
        PlanNodeEnum::UpdateEdges(update) => (
            update
                .updates()
                .iter()
                .map(|value| {
                    Ok(vec![
                        contextual_to_value(&value.src)?,
                        contextual_to_value(&value.dst)?,
                    ])
                })
                .collect::<Result<Vec<_>, PlanBuildError>>()?,
            vec!["src".to_string(), "dst".to_string()],
        ),
        PlanNodeEnum::DeleteTags(delete) => (
            delete
                .vertex_ids()
                .iter()
                .map(|value| contextual_to_value(value).map(|value| vec![value]))
                .collect::<Result<Vec<_>, _>>()?,
            vec!["vid".to_string()],
        ),
        _ => {
            return Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "not a standalone write node",
            ));
        }
    };
    Ok(SourceSpec::ScanVertices { rows, col_names })
}

// ── Unary spec builders ───────────────────────────────────────────────────────

pub(super) fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, PlanBuildError> {
    expr.get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "ContextualExpression",
            0,
            format!("{:?}", expr),
            "Failed to get expression",
        )
    })
}

pub(super) fn build_filter_spec(
    node: &crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode,
) -> Result<UnarySpec, PlanBuildError> {
    let condition = node.condition();
    let predicate = contextual_to_expression(condition)?;
    Ok(UnarySpec::Filter { predicate })
}

pub(super) fn build_project_spec(
    node: &crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode,
) -> Result<UnarySpec, PlanBuildError> {
    let columns = node.columns();
    let output_expressions: Vec<Expression> = columns
        .iter()
        .map(|col| contextual_to_expression(&col.expression))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UnarySpec::Project {
        output_expressions,
        output_col_names: node.col_names().to_vec(),
    })
}

pub(super) fn build_limit_spec(
    node: &crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode,
) -> Result<UnarySpec, PlanBuildError> {
    let offset = u32::try_from(node.offset()).map_err(|_| {
        PlanBuildError::missing_value("Limit", node.id(), "offset", "Limit offset must fit in u32")
    })?;
    let limit = u32::try_from(node.count()).map_err(|_| {
        PlanBuildError::missing_value("Limit", node.id(), "count", "Limit count must fit in u32")
    })?;
    Ok(UnarySpec::Limit { offset, limit })
}

pub(super) fn build_sample_spec(
    node: &crate::query::planning::plan::core::nodes::operation::sample_node::SampleNode,
) -> Result<UnarySpec, PlanBuildError> {
    let count = if node.count() > 0 {
        node.count() as u64
    } else {
        return Err(PlanBuildError::missing_value(
            "Sample",
            node.id(),
            "count",
            "Sample count must be positive",
        ));
    };
    Ok(UnarySpec::Sample { count })
}

pub(super) fn build_remove_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::RemoveNode,
) -> Result<UnarySpec, PlanBuildError> {
    let columns_to_remove: Vec<String> = node
        .remove_items()
        .iter()
        .map(|(col, _)| col.clone())
        .collect();
    Ok(UnarySpec::Remove { columns_to_remove })
}

pub(super) fn build_assign_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode,
) -> Result<UnarySpec, PlanBuildError> {
    let assignments: Vec<(String, Expression)> = node
        .assignments()
        .iter()
        .filter_map(|(name, expr)| expr.get_expression().map(|e| (name.clone(), e)))
        .collect();
    Ok(UnarySpec::Assign { assignments })
}

pub(super) fn build_unwind_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode,
) -> Result<UnarySpec, PlanBuildError> {
    Ok(UnarySpec::Unwind {
        unwind_column: node.alias().to_string(),
    })
}

// ── Blocking spec builders ────────────────────────────────────────────────────

pub(super) fn build_sort_spec(
    node: &crate::query::planning::plan::core::nodes::operation::sort_node::SortNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let sort_items = node.sort_items();
    if sort_items.is_empty() {
        return Err(PlanBuildError::unsupported(
            "Sort",
            node.id(),
            "empty sort items",
        ));
    }
    let (sort_expressions, sort_directions) = sort_items_to_expressions(sort_items)?;
    Ok(BlockingSpec::Sort {
        sort_expressions,
        sort_directions,
    })
}

pub(super) fn build_aggregate_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let group_keys = node.group_keys();
    let group_by_expressions: Vec<Expression> = group_keys
        .iter()
        .map(|key| Expression::Variable(key.clone()))
        .collect();
    let agg_functions = node.aggregation_functions();
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
                AggregateFunction::Count(None) => Expression::Literal(crate::core::Value::Int(1)),
                _ => Expression::Literal(crate::core::Value::Int(1)),
            };
            (func.clone(), expr)
        })
        .collect();
    Ok(BlockingSpec::Aggregate {
        group_by_expressions,
        aggregate_functions,
        output_col_names: node.col_names().to_vec(),
    })
}

pub(super) fn build_topn_spec(
    node: &crate::query::planning::plan::core::nodes::operation::sort_node::TopNNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let sort_items = node.sort_items();
    let (sort_expressions, sort_directions) = sort_items_to_expressions(sort_items)?;
    Ok(BlockingSpec::TopN {
        n: node.limit() as u32,
        sort_expressions,
        sort_directions,
    })
}

pub(super) fn build_window_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::window_node::WindowNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let window_functions = node.window_functions();
    let mut window_exprs = Vec::new();
    let mut partition_by_exprs = Vec::new();
    let mut order_by_exprs = Vec::new();
    let mut order_by_directions = Vec::new();
    for wf in window_functions {
        window_exprs.push(Expression::WindowFunction {
            name: wf.name.clone(),
            args: wf.args.clone(),
            over_partition_by: wf.partition_by.clone(),
            over_order_by: wf.order_by.clone(),
            over_order_desc: wf.order_desc.clone(),
        });
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
                        crate::query::executor::streaming::executor::SortDirection::Descending
                    } else {
                        crate::query::executor::streaming::executor::SortDirection::Ascending
                    }
                })
                .collect();
        }
    }
    Ok(BlockingSpec::WindowFunction {
        window_exprs,
        partition_by_exprs,
        order_by_exprs,
        order_by_directions,
    })
}

pub(super) fn sort_items_to_expressions(
    items: &[crate::query::planning::plan::core::nodes::operation::sort_node::SortItem],
) -> Result<
    (
        Vec<Expression>,
        Vec<crate::query::executor::streaming::executor::SortDirection>,
    ),
    PlanBuildError,
> {
    let mut expressions = Vec::new();
    let mut directions = Vec::new();
    for item in items {
        expressions.push(item.expression.clone());
        let direction = match item.direction {
            crate::core::types::graph_schema::OrderDirection::Asc => {
                crate::query::executor::streaming::executor::SortDirection::Ascending
            }
            crate::core::types::graph_schema::OrderDirection::Desc => {
                crate::query::executor::streaming::executor::SortDirection::Descending
            }
        };
        directions.push(direction);
    }
    Ok((expressions, directions))
}

// ── Graph spec builders ───────────────────────────────────────────────────────

pub(super) fn build_expand_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::traversal_node::ExpandNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::Expand {
        edge_types: node.edge_types().to_vec(),
        direction: node.direction(),
        filter_expr: node.filter().map(contextual_to_expression).transpose()?,
        col_names: node.col_names().to_vec(),
    })
}

pub(super) fn build_expand_all_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::ExpandAll {
        edge_types: node.edge_types().to_vec(),
        direction: match node.direction().to_lowercase().as_str() {
            "out" | "outgoing" => crate::core::EdgeDirection::Out,
            "in" | "incoming" => crate::core::EdgeDirection::In,
            _ => crate::core::EdgeDirection::Both,
        },
        filter_expr: node.filter().map(contextual_to_expression).transpose()?,
        col_names: node.col_names().to_vec(),
        src_vids: node.src_vids().to_vec(),
        step_limit: node.step_limit().unwrap_or(1),
    })
}

pub(super) fn build_traverse_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::traversal_node::TraverseNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::Traverse {
        edge_types: node.edge_types().to_vec(),
        direction: node.direction(),
        min_depth: node.min_steps(),
        max_depth: node.max_steps(),
        filter_expr: node
            .e_filter()
            .or_else(|| node.v_filter())
            .map(contextual_to_expression)
            .transpose()?,
    })
}

pub(super) fn build_bi_expand_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::traversal_node::BiExpandNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::BiExpand {
        edge_types: node.edge_types().to_vec(),
        direction: node.left_direction(),
    })
}

pub(super) fn build_bi_traverse_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::BiTraverse {
        edge_types: node.edge_types().to_vec(),
        direction: node.left_direction(),
        min_depth: node.min_hops() as u32,
        max_depth: node.max_hops() as u32,
    })
}

// ── Recursive fragment spec builders ─────────────────────────────────────────

pub(super) fn build_shortest_path_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::path_algorithms::ShortestPathNode,
    _exec_ctx: &ExecutionContext,
) -> Result<RecursiveFragmentSpec, PlanBuildError> {
    if node.weight_expression().is_some() || node.heuristic_expression().is_some() {
        return Err(PlanBuildError::capability(
            "weighted_shortest_path",
            "Weighted shortest path is not supported by the streaming executor",
        ));
    }
    Ok(RecursiveFragmentSpec::ShortestPath {
        edge_types: node.edge_types().to_vec(),
        direction: if node.no_reverse() {
            crate::core::EdgeDirection::Out
        } else {
            crate::core::EdgeDirection::Both
        },
        max_depth: node.max_step(),
        start_vertices: node.start_vertex_ids().to_vec(),
        target_vertices: node.end_vertex_ids().to_vec(),
    })
}

pub(super) fn build_multi_shortest_path_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::path_algorithms::MultiShortestPathNode,
    _exec_ctx: &ExecutionContext,
) -> Result<RecursiveFragmentSpec, PlanBuildError> {
    Ok(RecursiveFragmentSpec::MultiShortestPath {
        edge_types: node.edge_types().to_vec(),
        direction: node.direction(),
        max_depth: node.steps(),
        left_vertex_column: node.left_vid_var().to_string(),
        right_vertex_column: node.right_vid_var().to_string(),
        single_shortest: node.single_shortest(),
    })
}

pub(super) fn build_bfs_shortest_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::path_algorithms::BFSShortestNode,
    _exec_ctx: &ExecutionContext,
) -> Result<RecursiveFragmentSpec, PlanBuildError> {
    Ok(RecursiveFragmentSpec::BFSShortest {
        edge_types: node.edge_types().to_vec(),
        direction: if node.reverse() {
            crate::core::EdgeDirection::In
        } else {
            crate::core::EdgeDirection::Both
        },
        max_depth: node.steps(),
        allow_loops: node.with_loop(),
    })
}

pub(super) fn build_all_paths_spec(
    node: &crate::query::planning::plan::core::nodes::traversal::path_algorithms::AllPathsNode,
    _exec_ctx: &ExecutionContext,
) -> Result<RecursiveFragmentSpec, PlanBuildError> {
    if node.max_hop() < node.min_hop() {
        return Err(PlanBuildError::missing_value(
            "AllPaths",
            node.id(),
            "max_hop",
            "AllPaths max hop must not be smaller than min hop",
        ));
    }
    let offset = usize::try_from(node.offset()).map_err(|_| {
        PlanBuildError::missing_value(
            "AllPaths",
            node.id(),
            "offset",
            "offset must be non-negative",
        )
    })?;
    let limit = if node.limit() < 0 {
        None
    } else {
        Some(usize::try_from(node.limit()).map_err(|_| {
            PlanBuildError::missing_value(
                "AllPaths",
                node.id(),
                "limit",
                "limit does not fit in usize",
            )
        })?)
    };
    Ok(RecursiveFragmentSpec::AllPaths {
        edge_types: node.edge_types().to_vec(),
        direction: crate::core::EdgeDirection::Both,
        min_depth: node.min_hop(),
        max_depth: node.max_hop(),
        acyclic: node.is_acyclic(),
        limit,
        offset,
        start_vertices: node
            .start_vertex_ids()
            .iter()
            .copied()
            .map(crate::core::Value::from)
            .collect(),
        target_vertices: node
            .end_vertex_ids()
            .iter()
            .copied()
            .map(crate::core::Value::from)
            .collect(),
    })
}

// ── Join spec builders ────────────────────────────────────────────────────────

pub(super) fn build_inner_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::InnerJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_left_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::LeftJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::LeftJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_hash_inner_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::InnerJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_hash_left_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::HashLeftJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::LeftJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_right_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::RightJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::RightJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_full_outer_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::FullOuterJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::FullOuterJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_semi_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::SemiJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        node.right_input().col_names(),
        JoinSpec::SemiJoin {
            join_condition: None,
        },
    )
}

pub(super) fn build_join_with_keys(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    _right_col_names: &[String],
    default: JoinSpec,
) -> Result<JoinSpec, PlanBuildError> {
    if hash_keys.is_empty() || probe_keys.is_empty() || hash_keys.len() != probe_keys.len() {
        return Ok(default);
    }
    let left_first = hash_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", hash_keys[0]),
            "Failed to resolve hash key expression",
        )
    })?;
    let right_first = probe_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", probe_keys[0]),
            "Failed to resolve probe key expression",
        )
    })?;
    let mut condition = Expression::Binary {
        left: Box::new(left_first),
        op: crate::core::types::operators::BinaryOperator::Equal,
        right: Box::new(right_first),
    };
    for i in 1..hash_keys.len() {
        let left = hash_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", hash_keys[i]),
                "Failed to resolve hash key expression",
            )
        })?;
        let right = probe_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", probe_keys[i]),
                "Failed to resolve probe key expression",
            )
        })?;
        let eq = Expression::Binary {
            left: Box::new(left),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(right),
        };
        condition = Expression::Binary {
            left: Box::new(condition),
            op: crate::core::types::operators::BinaryOperator::And,
            right: Box::new(eq),
        };
    }
    match default {
        JoinSpec::InnerJoin { .. } => Ok(JoinSpec::InnerJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::LeftJoin { .. } => Ok(JoinSpec::LeftJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::RightJoin { .. } => Ok(JoinSpec::RightJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::FullOuterJoin { .. } => Ok(JoinSpec::FullOuterJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::SemiJoin { .. } => Ok(JoinSpec::SemiJoin {
            join_condition: Some(condition),
        }),
        _ => Ok(default),
    }
}

// ── Set/Apply spec builders ───────────────────────────────────────────────────

pub(super) fn build_pattern_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::PatternApply {
        key_expressions: node
            .key_cols()
            .iter()
            .map(contextual_to_expression)
            .collect::<Result<Vec<_>, _>>()?,
        anti: node.is_anti_predicate(),
    })
}

pub(super) fn build_rollup_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::RollUpApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::RollUpApply {
        compare_columns: node.compare_cols().to_vec(),
        collect_column: node.collect_col().map(|column| column.to_string()),
    })
}

pub(super) fn build_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::Apply {
        kind: match node.apply_kind() {
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Semi => crate::query::executor::streaming::operators::spec::ApplyKind::Semi,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Anti => crate::query::executor::streaming::operators::spec::ApplyKind::Anti,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Single => crate::query::executor::streaming::operators::spec::ApplyKind::Single,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::All => crate::query::executor::streaming::operators::spec::ApplyKind::All,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Standard => crate::query::executor::streaming::operators::spec::ApplyKind::Standard,
        },
        correlated_columns: node.correlated_cols().to_vec(),
    })
}

// ── Sink spec builders ────────────────────────────────────────────────────────

pub(super) fn build_insert_vertices_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::insert_nodes::InsertVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::InsertVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_properties: std::iter::once((
            "vid".to_string(),
            Expression::Variable("vid".to_string()),
        ))
        .chain(
            node.tags()
                .iter()
                .flat_map(|tag| tag.prop_names.iter())
                .map(|name| (name.clone(), Expression::Variable(name.clone()))),
        )
        .collect(),
        tags: node.tag_names(),
    })
}

pub(super) fn build_insert_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::insert_nodes::InsertEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::InsertEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type: node.edge_name().to_string(),
        edge_properties: node
            .prop_names()
            .iter()
            .map(|name| (name.clone(), Expression::Variable(name.clone())))
            .collect(),
    })
}

pub(super) fn build_delete_vertices_spec(
    _node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::DeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_id_col: "vid".to_string(),
    })
}

pub(super) fn build_delete_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let edge_type = node.edge_type().unwrap_or("").to_string();
    Ok(SinkSpec::DeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type,
    })
}

pub(super) fn build_delete_tags_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteTagsNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::DeleteTags {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        tag_names: node.tag_names().to_vec(),
        vertex_ids: None,
    })
}

pub(super) fn build_pipe_delete_vertices_spec(
    _node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::PipeDeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_id_col: "vid".to_string(),
    })
}

pub(super) fn build_pipe_delete_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let edge_type = node.edge_type().unwrap_or("").to_string();
    Ok(SinkSpec::PipeDeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type,
    })
}

pub(super) fn build_update_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    use crate::query::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
    match node.info() {
        UpdateTargetType::Vertex(info) => Ok(SinkSpec::UpdateVertices {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            tag_name: info.tag_name.clone().unwrap_or_default(),
            updates: info
                .properties
                .iter()
                .filter_map(|(name, value)| value.get_expression().map(|expr| (name.clone(), expr)))
                .collect(),
        }),
        UpdateTargetType::Edge(info) => Ok(SinkSpec::UpdateEdges {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            src_col: "src".to_string(),
            dst_col: "dst".to_string(),
            edge_type: info.edge_type.clone().unwrap_or_default(),
            updates: info
                .properties
                .iter()
                .filter_map(|(name, value)| value.get_expression().map(|expr| (name.clone(), expr)))
                .collect(),
        }),
    }
}

pub(super) fn build_update_vertices_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let tag_name = node
        .updates()
        .first()
        .and_then(|update| update.tag_name.clone())
        .unwrap_or_default();
    Ok(SinkSpec::UpdateVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        tag_name,
        updates: node
            .updates()
            .iter()
            .flat_map(|update| update.properties.iter())
            .map(|(name, value)| (name.clone(), value.clone().into_expression()))
            .collect(),
    })
}

pub(super) fn build_update_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::UpdateEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type: node
            .updates()
            .first()
            .and_then(|update| update.edge_type.clone())
            .unwrap_or_default(),
        updates: node
            .updates()
            .iter()
            .flat_map(|update| update.properties.iter())
            .map(|(name, value)| (name.clone(), value.clone().into_expression()))
            .collect(),
    })
}

// ── DDL spec builders ─────────────────────────────────────────────────────────

pub(super) fn build_space_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::SpaceManage {
        command: node.clone(),
    })
}

pub(super) fn build_tag_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::TagManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: node.clone(),
    })
}

pub(super) fn build_edge_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::EdgeManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: node.clone(),
    })
}

pub(super) fn build_index_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::IndexManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: node.clone(),
    })
}

pub(super) fn build_delete_index_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteIndexNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::DeleteIndex {
        space_name: node.info().space_name.clone(),
        index_name: node.info().index_name.clone(),
    })
}

pub(super) fn build_user_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::UserManage {
        command: node.clone(),
    })
}

// ── Fulltext spec builders ────────────────────────────────────────────────────

pub(super) fn build_fulltext_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: node.clone(),
    })
}

pub(super) fn build_fulltext_search_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        search_query: fulltext_query_to_string(&node.query),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

pub(super) fn build_fulltext_lookup_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        search_query: node.query.clone(),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

pub(super) fn build_match_fulltext_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::MatchFulltext {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        match_expr: Expression::Literal(crate::core::Value::String(format!(
            "{}:{}",
            node.fulltext_condition.field, node.fulltext_condition.query
        ))),
        match_field: Some(node.field_name.clone()),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

// ── Vector spec builders ──────────────────────────────────────────────────────

pub(super) fn build_vector_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: node.clone(),
    })
}

#[cfg(feature = "qdrant")]
pub(super) fn build_vector_search_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        index_name: node.index_name.clone(),
        query_vector: vector_query_to_vec(&node.query),
        top_k: node.limit as u32,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

#[cfg(feature = "qdrant")]
pub(super) fn build_vector_lookup_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        index_name: node.index_name.clone(),
        lookup_key: Expression::Literal(crate::core::Value::String(node.query.query_data.clone())),
    })
}

#[cfg(feature = "qdrant")]
pub(super) fn build_vector_match_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorMatch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        pattern: node.pattern.clone(),
        field: node.field.clone(),
        query_vector: vector_query_to_vec(&node.query),
        threshold: node.threshold,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        space_id: node.space_id,
    })
}

#[cfg(feature = "qdrant")]
fn vector_query_to_vec(expr: &crate::query::parser::ast::vector::VectorQueryExpr) -> Vec<f32> {
    serde_json::from_str(&expr.query_data).unwrap_or_default()
}
