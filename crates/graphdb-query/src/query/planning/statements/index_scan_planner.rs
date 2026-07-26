use crate::core::types::expr::Expression;
use crate::core::types::operators::BinaryOperator;
use crate::core::Value;
use crate::query::metadata::{IndexMetadata, MetadataContext};
use crate::query::parser::ast::pattern::NodePattern;
use crate::query::planning::plan::core::nodes::access::index_scan::{
    IndexLimit, IndexScanNode, ScanType,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;

pub fn try_create_index_scan_plan(
    node: &NodePattern,
    space_id: u64,
    _space_name: &str,
    var_name: &str,
    enable_index_optimization: bool,
    metadata_context: Option<&MetadataContext>,
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

    let suitable_index = find_suitable_index(metadata_ctx, tag_name, node, var_name)?;

    match suitable_index {
        Some((index, index_limits)) => {
            let mut index_scan_node = IndexScanNode::new(
                space_id,
                0,
                0,
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
) -> Result<Option<(IndexMetadata, Vec<IndexLimit>)>, PlannerError> {
    let tag_metadata = match metadata_ctx.get_tag_metadata(tag_name) {
        Some(meta) => meta,
        None => return Ok(None),
    };

    if tag_metadata.indexes.is_empty() {
        return Ok(None);
    }

    let filter_conditions = extract_filter_conditions(node, var_name);

    if filter_conditions.is_empty() {
        return Ok(None);
    }

    for index_name in &tag_metadata.indexes {
        if let Some(index_meta) = metadata_ctx.get_index_metadata(index_name) {
            for (field, op, value) in &filter_conditions {
                if &index_meta.field_name == field {
                    let index_limit = match op.as_str() {
                        "=" => Some(IndexLimit::equal(
                            field.clone(),
                            Value::string(value.clone()),
                        )),
                        ">" => Some(IndexLimit::range(
                            field.clone(),
                            Some(Value::string(value.clone())),
                            None::<Value>,
                            false,
                            false,
                        )),
                        "<" => Some(IndexLimit::range(
                            field.clone(),
                            None::<Value>,
                            Some(Value::string(value.clone())),
                            false,
                            false,
                        )),
                        ">=" => Some(IndexLimit::range(
                            field.clone(),
                            Some(Value::string(value.clone())),
                            None::<Value>,
                            true,
                            false,
                        )),
                        "<=" => Some(IndexLimit::range(
                            field.clone(),
                            None::<Value>,
                            Some(Value::string(value.clone())),
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

fn extract_filter_conditions(node: &NodePattern, var_name: &str) -> Vec<(String, String, String)> {
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

    conditions
}

fn extract_conditions_from_expression(
    expr: &Expression,
    var_name: &str,
    conditions: &mut Vec<(String, String, String)>,
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
                            if let Some(value_str) = value_to_index_string(lit) {
                                conditions.push((property.clone(), op_str.clone(), value_str));
                            }
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
                            if let Some(value_str) = value_to_index_string(lit) {
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
                    if let Some(value_str) = value_to_index_string(lit) {
                        conditions.push((key.clone(), "=".to_string(), value_str));
                    }
                }
            }
        }
        _ => {}
    }
}

fn value_to_index_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string()),
        Value::SmallInt(i) => Some(i.to_string()),
        Value::Int(i) => Some(i.to_string()),
        Value::BigInt(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Double(d) => Some(d.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
