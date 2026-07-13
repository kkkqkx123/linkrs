use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::SinkSpec;
use crate::query::executor::streaming::operator_spec::SourceSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

fn contextual_to_value(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Value, QueryError> {
    expr.constant_value().ok_or_else(|| {
        QueryError::execution("Expected constant value in plan node".to_string())
    })
}

fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, QueryError> {
    expr.get_expression().ok_or_else(|| {
        QueryError::execution("Failed to get expression from ContextualExpression".to_string())
    })
}

pub fn lower_write_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
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
                vertex_properties
                    .push((prop_name.clone(), Expression::Variable(prop_name.clone())));
            }
            for (vid_expr, tag_values) in insert_node.values() {
                let mut row = vec![contextual_to_value(vid_expr)?];
                for values in tag_values {
                    for value_expr in values {
                        row.push(contextual_to_value(value_expr)?);
                    }
                }
                rows.push(row);
            }
            let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                rows,
                col_names: scan_col_names,
            });
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::InsertVertices {
                    vertex_properties,
                    tags: insert_node.tag_names(),
                },
            ))
        }

        PlanNodeEnum::InsertEdges(insert_node) => {
            let mut rows = Vec::new();
            let prop_names = insert_node.prop_names();
            let mut scan_col_names =
                vec!["src".to_string(), "dst".to_string(), "rank".to_string()];
            scan_col_names.extend(prop_names.iter().cloned());
            for (src, dst, rank, props) in insert_node.edges() {
                let mut row = vec![
                    contextual_to_value(src)?,
                    contextual_to_value(dst)?,
                ];
                row.push(match rank {
                    Some(rank_expr) => contextual_to_value(rank_expr)?,
                    None => Value::BigInt(0),
                });
                for prop in props {
                    row.push(contextual_to_value(prop)?);
                }
                rows.push(row);
            }
            let edge_properties = prop_names
                .iter()
                .map(|prop| (prop.clone(), Expression::Variable(prop.clone())))
                .collect();
            let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                rows,
                col_names: scan_col_names,
            });
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::InsertEdges {
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    edge_type: insert_node.edge_name().to_string(),
                    edge_properties,
                },
            ))
        }

        PlanNodeEnum::UpdateVertices(update_node) => {
            let mut rows = Vec::new();
            let mut updates = Vec::new();
            for update in update_node.updates() {
                rows.push(vec![contextual_to_value(&update.vertex_id)?]);
                for (name, expr) in &update.properties {
                    updates.push((name.clone(), contextual_to_expression(expr)?));
                }
            }
            let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                rows,
                col_names: vec!["vid".to_string()],
            });
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::UpdateVertices { updates },
            ))
        }

        PlanNodeEnum::Update(update_node) => {
            use crate::query::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
            match update_node.info() {
                UpdateTargetType::Vertex(vinfo) => {
                    let updates: Vec<(String, Expression)> = vinfo
                        .properties
                        .iter()
                        .filter_map(|(k, v)| v.get_expression().map(|e| (k.clone(), e)))
                        .collect();
                    let row = vec![vinfo
                        .vertex_id
                        .constant_value()
                        .unwrap_or(Value::Null(crate::core::NullType::Null))];
                    let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                        rows: vec![row],
                        col_names: vec!["vid".to_string()],
                    });
                    Ok(PhysicalNode::Sink(
                        Box::new(source),
                        SinkSpec::UpdateVertices { updates },
                    ))
                }
                UpdateTargetType::Edge(einfo) => {
                    let updates: Vec<(String, Expression)> = einfo
                        .properties
                        .iter()
                        .filter_map(|(k, v)| v.get_expression().map(|e| (k.clone(), e)))
                        .collect();
                    let row = vec![
                        einfo
                            .src
                            .constant_value()
                            .unwrap_or(Value::Null(crate::core::NullType::Null)),
                        einfo
                            .dst
                            .constant_value()
                            .unwrap_or(Value::Null(crate::core::NullType::Null)),
                    ];
                    let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                        rows: vec![row],
                        col_names: vec!["src".to_string(), "dst".to_string()],
                    });
                    Ok(PhysicalNode::Sink(
                        Box::new(source),
                        SinkSpec::UpdateEdges {
                            src_col: "src".to_string(),
                            dst_col: "dst".to_string(),
                            edge_type: einfo.edge_type.clone().unwrap_or_default(),
                            updates,
                        },
                    ))
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
            let source = PhysicalNode::Source(SourceSpec::Start);
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::UpdateEdges {
                    src_col,
                    dst_col,
                    edge_type,
                    updates,
                },
            ))
        }

        PlanNodeEnum::DeleteVertices(delete_node) => {
            let rows = delete_node
                .vertex_ids()
                .iter()
                .map(|id| contextual_to_value(id).map(|value| vec![value]))
                .collect::<Result<Vec<_>, _>>()?;
            let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                rows,
                col_names: vec!["vid".to_string()],
            });
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::DeleteVertices {
                    vertex_id_col: "vid".to_string(),
                },
            ))
        }

        PlanNodeEnum::DeleteEdges(delete_node) => {
            let rows = delete_node
                .edges()
                .iter()
                .map(|(src, dst, _rank)| {
                    Ok(vec![
                        contextual_to_value(src)?,
                        contextual_to_value(dst)?,
                    ])
                })
                .collect::<Result<Vec<_>, QueryError>>()?;
            let source = PhysicalNode::Source(SourceSpec::ScanVertices {
                rows,
                col_names: vec!["src".to_string(), "dst".to_string()],
            });
            Ok(PhysicalNode::Sink(
                Box::new(source),
                SinkSpec::DeleteEdges {
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                },
            ))
        }

        PlanNodeEnum::PipeDeleteVertices(delete_node) => {
            let input_plan = delete_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Sink(
                Box::new(input_phys),
                SinkSpec::PipeDeleteVertices {
                    vertex_id_col: "vid".to_string(),
                },
            ))
        }

        PlanNodeEnum::PipeDeleteEdges(delete_node) => {
            let input_plan = delete_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Sink(
                Box::new(input_phys),
                SinkSpec::PipeDeleteEdges {
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                },
            ))
        }

        PlanNodeEnum::DeleteTags(delete_tags_node) => {
            Ok(PhysicalNode::Sink(
                Box::new(PhysicalNode::Source(SourceSpec::Start)),
                SinkSpec::DeleteTags {
                    tag_names: delete_tags_node.tag_names().to_vec(),
                    vertex_ids: None,
                },
            ))
        }

        _ => Err(QueryError::execution(format!(
            "lowering::writes does not handle node type: {}",
            node.name()
        ))),
    }
}
