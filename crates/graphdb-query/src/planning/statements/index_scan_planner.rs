use crate::metadata::{IndexMetadata, MetadataContext};
use crate::parser::ast::pattern::NodePattern;
use crate::planning::plan::core::nodes::access::index_scan::{IndexLimit, IndexScanNode, ScanType};
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use graphdb_core::types::expr::{ContextualExpression, Expression};
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::Value;

pub fn try_create_index_scan_plan(
    node: &NodePattern,
    space_id: u64,
    _space_name: &str,
    var_name: &str,
    enable_index_optimization: bool,
    metadata_context: Option<&MetadataContext>,
    where_expression: Option<&ContextualExpression>,
) -> Result<Option<SubPlan>, PlannerError> {
    if !enable_index_optimization {
        return Ok(None);
    }

    let metadata_ctx = match metadata_context {
        Some(ctx) => ctx,
        None => return Ok(None),
    };

    if node.labels.is_empty() {
        return Ok(None);
    }

    let tag_name = &node.labels[0];

    let suitable_index =
        find_suitable_index(metadata_ctx, tag_name, node, var_name, where_expression)?;

    match suitable_index {
        Some((tag_id, index, index_limits)) => {
            let mut index_scan_node = IndexScanNode::new(
                space_id,
                tag_id as i32,
                index.index_id,
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

fn find_suitable_index(
    metadata_ctx: &MetadataContext,
    tag_name: &str,
    node: &NodePattern,
    var_name: &str,
    where_expression: Option<&ContextualExpression>,
) -> Result<Option<(u32, IndexMetadata, Vec<IndexLimit>)>, PlannerError> {
    let tag_metadata = match metadata_ctx.get_tag_metadata(tag_name) {
        Some(meta) => meta,
        None => return Ok(None),
    };

    if tag_metadata.indexes.is_empty() {
        return Ok(None);
    }

    let filter_conditions = extract_filter_conditions(node, var_name, where_expression);

    if filter_conditions.is_empty() {
        return Ok(None);
    }

    for index_name in &tag_metadata.indexes {
        if let Some(index_meta) = metadata_ctx.get_index_metadata(index_name) {
            for (field, op, value) in &filter_conditions {
                if &index_meta.field_name == field {
                    let index_limit = match op.as_str() {
                        "=" => Some(IndexLimit::equal(field.clone(), value.clone())),
                        ">" => Some(IndexLimit::range(
                            field.clone(),
                            Some(value.clone()),
                            None::<Value>,
                            false,
                            false,
                        )),
                        "<" => Some(IndexLimit::range(
                            field.clone(),
                            None::<Value>,
                            Some(value.clone()),
                            false,
                            false,
                        )),
                        ">=" => Some(IndexLimit::range(
                            field.clone(),
                            Some(value.clone()),
                            None::<Value>,
                            true,
                            false,
                        )),
                        "<=" => Some(IndexLimit::range(
                            field.clone(),
                            None::<Value>,
                            Some(value.clone()),
                            false,
                            true,
                        )),
                        _ => None,
                    };

                    if let Some(limit) = index_limit {
                        return Ok(Some((tag_metadata.tag_id, index_meta.clone(), vec![limit])));
                    }
                }
            }
        }
    }

    Ok(None)
}

fn extract_filter_conditions(
    node: &NodePattern,
    var_name: &str,
    where_expression: Option<&ContextualExpression>,
) -> Vec<(String, String, Value)> {
    let mut conditions = Vec::new();

    if let Some(ref props) = node.properties {
        if let Some(expr_meta) = props.expression() {
            extract_conditions_from_expression(expr_meta.inner(), var_name, &mut conditions);
        }
    }

    for pred in &node.predicates {
        if let Some(expr_meta) = pred.expression() {
            extract_conditions_from_expression(expr_meta.inner(), var_name, &mut conditions);
        }
    }

    if let Some(where_expr) = where_expression {
        if let Some(expr_meta) = where_expr.expression() {
            extract_conditions_from_expression(expr_meta.inner(), var_name, &mut conditions);
        }
    }

    conditions
}

fn extract_conditions_from_expression(
    expr: &Expression,
    var_name: &str,
    conditions: &mut Vec<(String, String, Value)>,
) {
    match expr {
        Expression::Binary { left, op, right } => {
            if matches!(op, BinaryOperator::And) {
                extract_conditions_from_expression(left, var_name, conditions);
                extract_conditions_from_expression(right, var_name, conditions);
                return;
            }

            let op_str = op.to_string();

            if let Expression::Property { object, property } = left.as_ref() {
                if let Expression::Variable(obj_name) = object.as_ref() {
                    if obj_name == var_name {
                        if let Expression::Literal(lit) = right.as_ref() {
                            conditions.push((property.clone(), op_str.clone(), lit.clone()));
                        }
                    }
                }
            }

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
                            conditions.push((property.clone(), reversed_op, lit.clone()));
                        }
                    }
                }
            }
        }
        Expression::Map(pairs) => {
            for (key, value_expr) in pairs {
                if let Expression::Literal(lit) = value_expr {
                    conditions.push((key.clone(), "=".to_string(), lit.clone()));
                }
            }
        }
        _ => {}
    }
}
