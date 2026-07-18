use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::expression::functions::global_registry;
use crate::query::executor::streaming::operators::spec::SinkSpec;
use crate::query::executor::streaming::operators::spec::SourceSpec;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::executor::streaming::plan::properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

fn eval_expr_to_value(expr: &Expression) -> Option<Value> {
    match expr {
        Expression::Literal(value) => Some(value.clone()),
        Expression::Vector(data) => Some(Value::vector(data.clone())),
        Expression::List(elements) => {
            let vals: Option<Vec<Value>> = elements.iter().map(eval_expr_to_value).collect();
            vals.map(|v| Value::list(crate::core::value::list::List::from(v)))
        }
        Expression::Function { name, args } => {
            let arg_vals: Vec<Value> = args.iter().map(eval_expr_to_value).collect::<Option<Vec<_>>>()?;
            global_registry().execute(name, &arg_vals).ok()
        }
        Expression::Binary { left, op, right } => {
            let l = eval_expr_to_value(left)?;
            let r = eval_expr_to_value(right)?;
            crate::query::executor::expression::evaluator::operations::BinaryOperationEvaluator::evaluate(&l, op, &r).ok()
        }
        Expression::Unary { op, operand } => {
            let v = eval_expr_to_value(operand)?;
            crate::query::executor::expression::evaluator::operations::UnaryOperationEvaluator::evaluate(op, &v).ok()
        }
        _ => None,
    }
}

fn contextual_to_value(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Value, PlanBuildError> {
    if let Some(value) = expr.constant_value() {
        return Ok(value);
    }
    match expr.get_expression() {
        Some(Expression::Literal(value)) => Ok(value),
        Some(ref expr) => {
            if let Some(value) = eval_expr_to_value(expr) {
                return Ok(value);
            }
            Err(PlanBuildError::expression(
                "ContextualExpression",
                0,
                format!("{:?}", expr),
                "Standalone data modification requires constant values, got expression",
            ))
        }
        None => unreachable!(),
    }
}

fn require_space_name(context: &ExecutionContext) -> Result<String, PlanBuildError> {
    context.space_name.clone().ok_or_else(|| {
        PlanBuildError::missing_value(
            "DataModification",
            0,
            "space_name",
            "Space name is required for data modification operations",
        )
    })
}

pub fn build_write_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
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
            let source = PhysicalNode::Source(
                super::SYNTHETIC_START_NODE_ID,
                SourceSpec::ScanVertices {
                    rows,
                    col_names: scan_col_names,
                },
                PhysicalProperties::single_streaming(),
            );
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(source),
                SinkSpec::InsertVertices {
                    space_name: require_space_name(context)?,
                    vertex_properties,
                    tags: insert_node.tag_names(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::InsertEdges(insert_node) => {
            let mut rows = Vec::new();
            let prop_names = insert_node.prop_names();
            let mut scan_col_names = vec!["src".to_string(), "dst".to_string(), "rank".to_string()];
            scan_col_names.extend(prop_names.iter().cloned());
            for (src, dst, rank, props) in insert_node.edges() {
                let mut row = vec![contextual_to_value(src)?, contextual_to_value(dst)?];
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
            let source = PhysicalNode::Source(
                super::SYNTHETIC_START_NODE_ID,
                SourceSpec::ScanVertices {
                    rows,
                    col_names: scan_col_names,
                },
                PhysicalProperties::single_streaming(),
            );
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(source),
                SinkSpec::InsertEdges {
                    space_name: require_space_name(context)?,
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                    edge_type: insert_node.edge_name().to_string(),
                    edge_properties,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::UpdateVertices(update_node) => {
            let mut rows = Vec::new();
            let mut updates = Vec::new();
            for update in update_node.updates() {
                rows.push(vec![contextual_to_value(&update.vertex_id)?]);
                for (name, expr) in &update.properties {
                    updates.push((name.clone(), super::contextual_to_expression(expr)?));
                }
            }
            let source = PhysicalNode::Source(
                super::SYNTHETIC_START_NODE_ID,
                SourceSpec::ScanVertices {
                    rows,
                    col_names: vec!["vid".to_string()],
                },
                PhysicalProperties::single_streaming(),
            );
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(source),
                SinkSpec::UpdateVertices {
                    space_name: require_space_name(context)?,
                    updates,
                },
                PhysicalProperties::single_streaming(),
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
                    let source = PhysicalNode::Source(
                        super::SYNTHETIC_START_NODE_ID,
                        SourceSpec::ScanVertices {
                            rows: vec![row],
                            col_names: vec!["vid".to_string()],
                        },
                        PhysicalProperties::single_streaming(),
                    );
                    Ok(PhysicalNode::Sink(
                        node.id(),
                        Box::new(source),
                        SinkSpec::UpdateVertices {
                            space_name: require_space_name(context)?,
                            updates,
                        },
                        PhysicalProperties::single_streaming(),
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
                    let source = PhysicalNode::Source(
                        super::SYNTHETIC_START_NODE_ID,
                        SourceSpec::ScanVertices {
                            rows: vec![row],
                            col_names: vec!["src".to_string(), "dst".to_string()],
                        },
                        PhysicalProperties::single_streaming(),
                    );
                    Ok(PhysicalNode::Sink(
                        node.id(),
                        Box::new(source),
                        SinkSpec::UpdateEdges {
                            space_name: require_space_name(context)?,
                            src_col: "src".to_string(),
                            dst_col: "dst".to_string(),
                            edge_type: einfo.edge_type.clone().unwrap_or_default(),
                            updates,
                        },
                        PhysicalProperties::single_streaming(),
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
            let source = super::single_start_source();
            Ok(PhysicalNode::Sink(
                node.id(),
                source,
                SinkSpec::UpdateEdges {
                    space_name: require_space_name(context)?,
                    src_col,
                    dst_col,
                    edge_type,
                    updates,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::DeleteVertices(delete_node) => {
            let rows = delete_node
                .vertex_ids()
                .iter()
                .map(|id| contextual_to_value(id).map(|value| vec![value]))
                .collect::<Result<Vec<_>, _>>()?;
            let source = PhysicalNode::Source(
                super::SYNTHETIC_START_NODE_ID,
                SourceSpec::ScanVertices {
                    rows,
                    col_names: vec!["vid".to_string()],
                },
                PhysicalProperties::single_streaming(),
            );
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(source),
                SinkSpec::DeleteVertices {
                    space_name: require_space_name(context)?,
                    vertex_id_col: "vid".to_string(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::DeleteEdges(delete_node) => {
            let rows = delete_node
                .edges()
                .iter()
                .map(|(src, dst, _rank)| {
                    Ok(vec![contextual_to_value(src)?, contextual_to_value(dst)?])
                })
                .collect::<Result<Vec<_>, PlanBuildError>>()?;
            let source = PhysicalNode::Source(
                super::SYNTHETIC_START_NODE_ID,
                SourceSpec::ScanVertices {
                    rows,
                    col_names: vec!["src".to_string(), "dst".to_string()],
                },
                PhysicalProperties::single_streaming(),
            );
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(source),
                SinkSpec::DeleteEdges {
                    space_name: require_space_name(context)?,
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::PipeDeleteVertices(delete_node) => {
            let input_plan = delete_node.input();
            let input_phys = super::build_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(input_phys),
                SinkSpec::PipeDeleteVertices {
                    space_name: require_space_name(context)?,
                    vertex_id_col: "vid".to_string(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::PipeDeleteEdges(delete_node) => {
            let input_plan = delete_node.input();
            let input_phys = super::build_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Sink(
                node.id(),
                Box::new(input_phys),
                SinkSpec::PipeDeleteEdges {
                    space_name: require_space_name(context)?,
                    src_col: "src".to_string(),
                    dst_col: "dst".to_string(),
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::DeleteTags(delete_tags_node) => Ok(PhysicalNode::Sink(
            node.id(),
            super::single_start_source(),
            SinkSpec::DeleteTags {
                space_name: require_space_name(context)?,
                tag_names: delete_tags_node.tag_names().to_vec(),
                vertex_ids: None,
            },
            PhysicalProperties::single_streaming(),
        )),

        _ => Err(super::internal_routing_error(node, "writes")),
    }
}
