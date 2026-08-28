//! Source spec builders: scans and standalone write value sources.

use std::sync::Arc;

use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::SourceSpec;
use crate::executor::streaming::slot::SlotLayout;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub(in crate::executor::streaming::plan::arena_builder) fn build_source_spec(
    node: &PlanNodeEnum,
    exec_ctx: &ExecutionContext,
) -> Result<SourceSpec, PlanBuildError> {
    match node {
        PlanNodeEnum::Start(_) => Ok(SourceSpec::Start),
        PlanNodeEnum::Argument(arg_node) => Ok(SourceSpec::Argument {
            col_names: arg_node.col_names().to_vec(),
        }),
        PlanNodeEnum::ScanVertices(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|v| (v >= 0).then_some(v as usize));
            let projected_properties = scan_node.projected_properties().to_vec();
            let predicate = crate::planning::scan_predicate::extract_scan_predicates(
                scan_node.filter(),
                &projected_properties,
            );
            Ok(SourceSpec::StorageScanVertices {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                limit,
                col_names: scan_node.col_names().to_vec(),
                projected_properties,
                predicate,
                tag: scan_node.tag().map(|s| s.to_string()),
                partition_range: None,
            })
        }
        PlanNodeEnum::ScanEdges(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|v| (v >= 0).then_some(v as usize));
            let projected_properties = scan_node.projected_properties().to_vec();
            let predicate = crate::planning::scan_predicate::extract_scan_predicates(
                scan_node.filter(),
                &projected_properties,
            );
            Ok(SourceSpec::StorageScanEdges {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                limit,
                edge_type: scan_node.edge_type().map(|s| s.to_string()),
                col_names: scan_node.col_names().to_vec(),
                projected_properties,
                predicate,
                partition_range: None,
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
                                .map(|s| graphdb_core::Value::string(s.trim()))
                                .collect::<Vec<_>>(),
                        )
                    }
                });
            Ok(SourceSpec::GetVertices {
                space_name: get_node.space_name().to_string(),
                vertex_ids,
                projected_properties: get_node.projected_properties().to_vec(),
                col_names: get_node.col_names().to_vec(),
            })
        }
        PlanNodeEnum::GetEdges(get_node) => Ok(SourceSpec::GetEdges {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            edge_type: Some(get_node.edge_type().to_string()),
            src: Some(get_node.src().to_string()),
            dst: Some(get_node.dst().to_string()),
            rank: 0,
            projected_properties: get_node.projected_properties().to_vec(),
        }),
        PlanNodeEnum::GetNeighbors(get_node) => Ok(SourceSpec::GetNeighbors {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            direction: get_node.direction().to_string(),
            projected_properties: get_node.projected_properties().to_vec(),
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
            let predicate = scan_node
                .scan_limits()
                .first()
                .map(index_limit_to_predicate)
                .unwrap_or(crate::executor::streaming::operators::spec::BoundIndexPredicate::Full);
            let projection = if scan_node.return_columns().is_empty() {
                crate::executor::streaming::operators::spec::IndexProjection::RowIdOnly
            } else {
                crate::executor::streaming::operators::spec::IndexProjection::Columns(
                    scan_node.return_columns().to_vec(),
                )
            };
            let col_names = scan_node.col_names().to_vec();
            // Widen the output layout with flat property columns (`{var}.{prop}`)
            // so covering index rows can hit the columnar `Property` fast path.
            let mut layout_names = col_names.clone();
            if let crate::executor::streaming::operators::spec::IndexProjection::Columns(columns) =
                &projection
            {
                if let Some(var) = col_names.first() {
                    layout_names.extend(columns.iter().map(|col| format!("{var}.{col}")));
                }
            }
            let output_layout = Arc::new(SlotLayout::from_names(&layout_names));
            Ok(SourceSpec::IndexScan {
                space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                index_name,
                index_id: scan_node.index_id(),
                predicate: Box::new(predicate),
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

/// Convert an `IndexLimit` (logical scan bound) into a `BoundIndexPredicate`.
fn index_limit_to_predicate(
    limit: &crate::planning::plan::core::nodes::access::index_scan::IndexLimit,
) -> crate::executor::streaming::operators::spec::BoundIndexPredicate {
    let column = limit.column.clone();
    use crate::executor::streaming::operators::spec::BoundIndexPredicate;
    match limit.scan_type {
        crate::planning::plan::core::nodes::access::index_scan::ScanType::Unique => {
            BoundIndexPredicate::Equal {
                column,
                value: limit
                    .begin_value
                    .clone()
                    .unwrap_or(graphdb_core::Value::Null(
                        graphdb_core::value::NullType::Null,
                    )),
            }
        }
        crate::planning::plan::core::nodes::access::index_scan::ScanType::Range => {
            BoundIndexPredicate::Range {
                column,
                begin: limit.begin_value.clone(),
                end: limit.end_value.clone(),
                include_begin: limit.include_begin,
                include_end: limit.include_end,
            }
        }
        crate::planning::plan::core::nodes::access::index_scan::ScanType::Prefix => {
            BoundIndexPredicate::Prefix {
                column,
                prefix: limit
                    .begin_value
                    .clone()
                    .unwrap_or(graphdb_core::Value::Null(
                        graphdb_core::value::NullType::Null,
                    )),
            }
        }
        crate::planning::plan::core::nodes::access::index_scan::ScanType::Full => {
            BoundIndexPredicate::Full
        }
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_standalone_write_source(
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
                let mut row = vec![vertex_id.clone()];
                for values in tag_values {
                    for value in values {
                        row.push(value.clone());
                    }
                }
                rows.push(row);
            }
            let mut names = vec!["vid".to_string()];
            names.extend(property_names);
            (rows, names)
        }
        PlanNodeEnum::InsertEdges(insert) => {
            use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
            let mut rows = Vec::with_capacity(insert.edges().len());
            for (src, dst, rank, properties) in insert.edges() {
                let mut row = vec![src.clone(), dst.clone()];
                let rank_expr = match rank {
                    Some(value) => value.clone(),
                    None => {
                        let id = src.context().register_expression(ExpressionMeta::new(
                            Expression::literal(graphdb_core::Value::BigInt(0)),
                        ));
                        ContextualExpression::new(id, src.context().clone())
                    }
                };
                row.push(rank_expr);
                for property in properties {
                    row.push(property.clone());
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
                .map(|value| vec![value.clone()])
                .collect::<Vec<_>>(),
            vec!["vid".to_string()],
        ),
        PlanNodeEnum::DeleteEdges(delete) => (
            delete
                .edges()
                .iter()
                .map(|(src, dst, _)| vec![src.clone(), dst.clone()])
                .collect::<Vec<_>>(),
            vec!["src".to_string(), "dst".to_string()],
        ),
        PlanNodeEnum::Update(update) => {
            use crate::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
            match update.info() {
                UpdateTargetType::Vertex(info) => {
                    (vec![vec![info.vertex_id.clone()]], vec!["vid".to_string()])
                }
                UpdateTargetType::Edge(info) => (
                    vec![vec![info.src.clone(), info.dst.clone()]],
                    vec!["src".to_string(), "dst".to_string()],
                ),
            }
        }
        PlanNodeEnum::UpdateVertices(update) => (
            update
                .updates()
                .iter()
                .map(|value| vec![value.vertex_id.clone()])
                .collect::<Vec<_>>(),
            vec!["vid".to_string()],
        ),
        PlanNodeEnum::UpdateEdges(update) => (
            update
                .updates()
                .iter()
                .map(|value| vec![value.src.clone(), value.dst.clone()])
                .collect::<Vec<_>>(),
            vec!["src".to_string(), "dst".to_string()],
        ),
        PlanNodeEnum::DeleteTags(delete) => (
            delete
                .vertex_ids()
                .iter()
                .map(|value| vec![value.clone()])
                .collect::<Vec<_>>(),
            vec!["vid".to_string()],
        ),
        PlanNodeEnum::CopyFrom(_) | PlanNodeEnum::CopyTo(_) => {
            // COPY is driven by file scan (FROM) or storage scan (TO), not
            // in-memory values: emit a single dummy row
            use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
            let expr_context =
                graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new();
            let ctx = std::sync::Arc::new(expr_context);
            let id = ctx.register_expression(ExpressionMeta::new(Expression::Literal(
                graphdb_core::Value::Null(graphdb_core::value::NullType::Null),
            )));
            let dummy = ContextualExpression::new(id, ctx);
            (vec![vec![dummy]], vec!["copy_dummy".to_string()])
        }
        _ => {
            return Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "not a standalone write node",
            ));
        }
    };
    Ok(SourceSpec::StandaloneValues {
        values: rows,
        col_names,
    })
}
