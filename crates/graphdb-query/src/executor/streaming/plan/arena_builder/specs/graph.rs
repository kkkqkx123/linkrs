//! Unary, blocking, graph-pattern and recursive-fragment spec builders.

use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    BlockingSpec, GraphSpec, RecursiveFragmentSpec, UnarySpec,
};
use crate::executor::streaming::subquery::SubqueryRunnerSpec;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, SingleInputNode};
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;

use super::contextual_to_expression;

pub(in crate::executor::streaming::plan::arena_builder) fn build_filter_spec(
    node: &crate::planning::plan::core::nodes::operation::filter_node::FilterNode,
    subquery_runners: Vec<SubqueryRunnerSpec>,
) -> Result<UnarySpec, PlanBuildError> {
    let condition = node.condition();
    let predicate = contextual_to_expression(condition)?;
    Ok(UnarySpec::Filter {
        predicate,
        subquery_runners,
    })
}

/// Build the storage-backed `AppendVertices` unary spec from the plan node.
///
/// The vertex to fetch is resolved per input row: the node's
/// `src_expression` takes precedence, falling back to `input_var` as a
/// row-column reference.  The appended columns are the flat
/// `{entity_var}.{prop}` names (or a single `{entity_var}` full-vertex
/// column when no properties are demanded).
pub(in crate::executor::streaming::plan::arena_builder) fn build_append_vertices_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<UnarySpec, PlanBuildError> {
    let entity_expr = if let Some(src) = node.src_expression() {
        contextual_to_expression(src)?
    } else if let Some(input_var) = node.input_var() {
        Expression::Variable(input_var.to_string())
    } else {
        return Err(PlanBuildError::capability(
            "append_vertices_entity",
            "AppendVertices requires an entity expression (src_expression or input_var)",
        ));
    };
    let entity_var = node
        .node_alias()
        .map(|s| s.to_string())
        .or_else(|| node.col_names().first().cloned())
        .unwrap_or_else(|| node.vertex_tag().to_string());
    let prop_names: Vec<String> = node
        .vertex_props()
        .iter()
        .flat_map(|tp| tp.props.iter().cloned())
        .collect();
    Ok(UnarySpec::AppendVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        entity_var,
        entity_expr,
        prop_names,
    })
}

/// Name of the single column a count-only expand emits (the per-chunk edge
/// count).  The count-only aggregate above rewrites `COUNT` to
/// `SUM(_expand_count)` over this column.
pub(in crate::executor::streaming::plan::arena_builder) const COUNT_ONLY_COLUMN: &str =
    "_expand_count";

/// Walk down from `node` through consecutive `Project` operators to find a
/// `count_only`-annotated `ExpandAll`.  Any other operator (including a
/// `Filter`) interrupts the walk and returns `None`.
///
/// The optimizer's `ExpandPushdown` batch only sets `count_only` when the
/// chain between the expand and its count-only aggregate consists of pure
/// `Project` pass-throughs, so a successful walk guarantees the count column
/// flows untouched into the aggregate.
pub(in crate::executor::streaming::plan::arena_builder) fn count_only_expand_below(
    node: &PlanNodeEnum,
) -> Option<crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode> {
    let mut current = node;
    loop {
        match current {
            PlanNodeEnum::Project(project) => current = project.input(),
            PlanNodeEnum::ExpandAll(expand) if expand.count_only() => return Some(expand.clone()),
            _ => return None,
        }
    }
}

/// Whether the aggregate node is a simple count-only aggregate: no GROUP BY
/// keys and all aggregate functions are COUNT.  This pattern allows the
/// upstream ExpandAll to skip materializing output rows entirely.
pub(in crate::executor::streaming::plan::arena_builder) fn is_count_only_aggregate(
    agg: &crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode,
) -> bool {
    agg.group_keys().is_empty()
        && !agg.aggregation_functions().is_empty()
        && agg
            .aggregation_functions()
            .iter()
            .all(|f| matches!(f, graphdb_core::types::operators::AggregateFunction::Count))
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_project_spec(
    node: &crate::planning::plan::core::nodes::operation::project_node::ProjectNode,
    subquery_runners: Vec<SubqueryRunnerSpec>,
) -> Result<UnarySpec, PlanBuildError> {
    // A Project sitting directly above a count_only expand only forwards the
    // aggregate argument of a count-only aggregate.  Replace it with a
    // pass-through of the expand's single count column so the count flows
    // unchanged into the rewritten `SUM(_expand_count)` aggregate.
    if let Some(expand) = count_only_expand_below(node.input()) {
        if project_forwards_expand_dst(node, &expand) {
            return Ok(UnarySpec::Project {
                output_expressions: vec![Expression::Variable(COUNT_ONLY_COLUMN.to_string())],
                output_col_names: vec![COUNT_ONLY_COLUMN.to_string()],
                subquery_runners,
            });
        }
    }
    let columns = node.columns();
    let output_expressions: Vec<Expression> = columns
        .iter()
        .map(|col| contextual_to_expression(&col.expression))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UnarySpec::Project {
        output_expressions,
        output_col_names: node.col_names().to_vec(),
        subquery_runners,
    })
}

/// True when every column of `project` is a bare reference to the count-only
/// expand's destination variable (the aggregate argument being forwarded).
fn project_forwards_expand_dst(
    project: &crate::planning::plan::core::nodes::operation::project_node::ProjectNode,
    expand: &crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode,
) -> bool {
    let Some(dst_var) = expand.col_names().get(2) else {
        return false;
    };
    !project.columns().is_empty()
        && project.columns().iter().all(|col| {
            col.expression
                .expression()
                .and_then(|meta| {
                    if let Expression::Variable(var) = meta.inner() {
                        Some(var.as_str() == dst_var)
                    } else {
                        None
                    }
                })
                .unwrap_or(false)
        })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_limit_spec(
    node: &crate::planning::plan::core::nodes::operation::sort_node::LimitNode,
) -> Result<UnarySpec, PlanBuildError> {
    let offset = u32::try_from(node.offset()).map_err(|_| {
        PlanBuildError::missing_value("Limit", node.id(), "offset", "Limit offset must fit in u32")
    })?;
    let limit = u32::try_from(node.count()).map_err(|_| {
        PlanBuildError::missing_value("Limit", node.id(), "count", "Limit count must fit in u32")
    })?;
    Ok(UnarySpec::Limit { offset, limit })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_sample_spec(
    node: &crate::planning::plan::core::nodes::operation::sample_node::SampleNode,
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_remove_spec(
    node: &crate::planning::plan::core::nodes::graph_operations::graph_operations_node::RemoveNode,
) -> Result<UnarySpec, PlanBuildError> {
    let columns_to_remove: Vec<String> = node
        .remove_items()
        .iter()
        .map(|(col, _)| col.clone())
        .collect();
    Ok(UnarySpec::Remove { columns_to_remove })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_assign_spec(
    node: &crate::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode,
    subquery_runners: Vec<SubqueryRunnerSpec>,
) -> Result<UnarySpec, PlanBuildError> {
    let assignments: Vec<(String, Expression)> = node
        .assignments()
        .iter()
        .filter_map(|(name, expr)| expr.get_expression().map(|e| (name.clone(), e)))
        .collect();
    Ok(UnarySpec::Assign {
        assignments,
        subquery_runners,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_unwind_spec(
    node: &crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode,
) -> Result<UnarySpec, PlanBuildError> {
    Ok(UnarySpec::Unwind {
        unwind_column: node.alias().to_string(),
        list_expression: node.list_expression().get_expression(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_flatten_spec(
    node: &crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode,
) -> Result<UnarySpec, PlanBuildError> {
    Ok(UnarySpec::Flatten {
        group_pos: node.group_pos(),
    })
}

// ── Blocking spec builders ────────────────────────────────────────────────────

pub(in crate::executor::streaming::plan::arena_builder) fn build_sort_spec(
    node: &crate::planning::plan::core::nodes::operation::sort_node::SortNode,
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_aggregate_spec(
    node: &crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let group_keys = node.group_keys();
    let group_by_expressions: Vec<Expression> = group_keys
        .iter()
        .map(|key| Expression::Variable(key.clone()))
        .collect();
    let count_only =
        is_count_only_aggregate(node) && count_only_expand_below(node.input()).is_some();
    let agg_functions = node.aggregation_functions();
    let agg_args = node.aggregation_args();
    let aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)> = agg_functions
        .iter()
        .enumerate()
        .map(|(i, func)| {
            if count_only {
                // The input is a count_only expand emitting one per-chunk edge
                // count per chunk.  Sum those counts instead of counting rows.
                return (
                    AggregateFunction::Sum,
                    vec![Expression::Variable(COUNT_ONLY_COLUMN.to_string())],
                );
            }
            let args = agg_args.get(i).cloned().unwrap_or_default();
            (*func, args)
        })
        .collect();
    Ok(BlockingSpec::Aggregate {
        group_by_expressions,
        aggregate_functions,
        output_col_names: node.col_names().to_vec(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_topn_spec(
    node: &crate::planning::plan::core::nodes::operation::sort_node::TopNNode,
) -> Result<BlockingSpec, PlanBuildError> {
    let sort_items = node.sort_items();
    let (sort_expressions, sort_directions) = sort_items_to_expressions(sort_items)?;
    Ok(BlockingSpec::TopN {
        n: node.limit() as u32,
        sort_expressions,
        sort_directions,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_window_spec(
    node: &crate::planning::plan::core::nodes::graph_operations::window_node::WindowNode,
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
                        crate::executor::streaming::executor::SortDirection::Descending
                    } else {
                        crate::executor::streaming::executor::SortDirection::Ascending
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

pub(in crate::executor::streaming::plan::arena_builder) fn sort_items_to_expressions(
    items: &[crate::planning::plan::core::nodes::operation::sort_node::SortItem],
) -> Result<
    (
        Vec<Expression>,
        Vec<crate::executor::streaming::executor::SortDirection>,
    ),
    PlanBuildError,
> {
    let mut expressions = Vec::new();
    let mut directions = Vec::new();
    for item in items {
        expressions.push(item.expression.clone());
        let direction = match item.direction {
            graphdb_core::types::graph_schema::OrderDirection::Asc => {
                crate::executor::streaming::executor::SortDirection::Ascending
            }
            graphdb_core::types::graph_schema::OrderDirection::Desc => {
                crate::executor::streaming::executor::SortDirection::Descending
            }
        };
        directions.push(direction);
    }
    Ok((expressions, directions))
}

// ── Graph spec builders ───────────────────────────────────────────────────────

pub(in crate::executor::streaming::plan::arena_builder) fn build_expand_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::ExpandNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::Expand {
        edge_types: node.edge_types().to_vec(),
        direction: node.direction(),
        filter_expr: node.filter().map(contextual_to_expression).transpose()?,
        col_names: node.col_names().to_vec(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_expand_all_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    build_expand_all_spec_with_flags(node, _exec_ctx, node.count_only())
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_expand_all_spec_with_flags(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode,
    _exec_ctx: &ExecutionContext,
    count_only: bool,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::ExpandAll {
        edge_types: node.edge_types().to_vec(),
        direction: match node.direction().to_lowercase().as_str() {
            "out" | "outgoing" => graphdb_core::EdgeDirection::Out,
            "in" | "incoming" => graphdb_core::EdgeDirection::In,
            _ => graphdb_core::EdgeDirection::Both,
        },
        filter_expr: node.filter().map(contextual_to_expression).transpose()?,
        col_names: node.col_names().to_vec(),
        src_vids: node.src_vids().to_vec(),
        step_limit: node.step_limit().unwrap_or(1),
        count_only,
        emit_raw_ids: node.id_only() || node.count_only(),
        lightweight_source: node.lightweight_source(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_traverse_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::TraverseNode,
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_bi_expand_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::BiExpandNode,
    _exec_ctx: &ExecutionContext,
) -> Result<GraphSpec, PlanBuildError> {
    Ok(GraphSpec::BiExpand {
        edge_types: node.edge_types().to_vec(),
        direction: node.left_direction(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_bi_traverse_spec(
    node: &crate::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNode,
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_shortest_path_spec(
    node: &crate::planning::plan::core::nodes::traversal::path_algorithms::ShortestPathNode,
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
            graphdb_core::EdgeDirection::Out
        } else {
            graphdb_core::EdgeDirection::Both
        },
        max_depth: node.max_step(),
        start_vertices: node.start_vertex_ids().to_vec(),
        target_vertices: node.end_vertex_ids().to_vec(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_multi_shortest_path_spec(
    node: &crate::planning::plan::core::nodes::traversal::path_algorithms::MultiShortestPathNode,
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_bfs_shortest_spec(
    node: &crate::planning::plan::core::nodes::traversal::path_algorithms::BFSShortestNode,
    _exec_ctx: &ExecutionContext,
) -> Result<RecursiveFragmentSpec, PlanBuildError> {
    Ok(RecursiveFragmentSpec::BFSShortest {
        edge_types: node.edge_types().to_vec(),
        direction: if node.reverse() {
            graphdb_core::EdgeDirection::In
        } else {
            graphdb_core::EdgeDirection::Both
        },
        max_depth: node.steps(),
        allow_loops: node.with_loop(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_all_paths_spec(
    node: &crate::planning::plan::core::nodes::traversal::path_algorithms::AllPathsNode,
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
        direction: node.direction(),
        min_depth: node.min_hop(),
        max_depth: node.max_hop(),
        acyclic: node.is_acyclic(),
        limit,
        offset,
        start_vertices: node
            .start_vertex_ids()
            .iter()
            .copied()
            .map(graphdb_core::Value::from)
            .collect(),
        target_vertices: node
            .end_vertex_ids()
            .iter()
            .copied()
            .map(graphdb_core::Value::from)
            .collect(),
    })
}
