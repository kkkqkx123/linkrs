use std::sync::Arc;

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::types::Expression;

use super::bound::{BoundAggregateCall, BoundExpression};

pub(crate) fn bound_expr_to_contextual(
    bound: &BoundExpression,
    ctx: &Arc<ExpressionAnalysisContext>,
) -> Result<ContextualExpression, String> {
    let expr = convert_bound_to_expression(bound)?;
    let meta = ExpressionMeta::new(expr);
    let id = ctx.register_expression(meta);
    ctx.set_type(&id, bound.return_type());
    Ok(ContextualExpression::new(id, ctx.clone()))
}

/// Convert a bound projection item into a `YieldColumn`, preserving the
/// resolved type in the contextual type slot and applying the shared default
/// alias rule when the item has no explicit alias.
pub(crate) fn bound_projection_to_yield_column(
    bound: &super::bound::BoundProjectionItem,
    ctx: &Arc<ExpressionAnalysisContext>,
) -> Result<graphdb_core::YieldColumn, String> {
    use graphdb_core::types::expr::expression_utils::generate_default_alias_from_contextual;
    // Same rule as `planning::statements::projection_util::default_projection_alias`
    // (called directly to avoid a binder -> planning dependency).
    let ctx_expr = bound_expr_to_contextual(&bound.expression, ctx)?;
    let alias = bound
        .alias
        .clone()
        .unwrap_or_else(|| generate_default_alias_from_contextual(&ctx_expr));
    Ok(graphdb_core::YieldColumn {
        expression: ctx_expr,
        alias,
    })
}

fn convert_bound_to_expression(bound: &BoundExpression) -> Result<Expression, String> {
    match bound {
        BoundExpression::Literal(v, _) => Ok(Expression::Literal(v.clone())),

        BoundExpression::Variable(name, _) => Ok(Expression::Variable(name.clone())),

        BoundExpression::Property {
            object, property, ..
        } => {
            let obj = convert_bound_to_expression(object)?;
            Ok(Expression::Property {
                object: Box::new(obj),
                property: property.clone(),
            })
        }

        BoundExpression::StructField { base, field, .. } => {
            let base = convert_bound_to_expression(base)?;
            Ok(Expression::StructField {
                base: Box::new(base),
                field: field.clone(),
            })
        }

        BoundExpression::BinaryOp {
            left, op, right, ..
        } => {
            let left = convert_bound_to_expression(left)?;
            let right = convert_bound_to_expression(right)?;
            Ok(Expression::Binary {
                left: Box::new(left),
                op: *op,
                right: Box::new(right),
            })
        }

        BoundExpression::UnaryOp { op, operand, .. } => {
            let operand = convert_bound_to_expression(operand)?;
            Ok(Expression::Unary {
                op: *op,
                operand: Box::new(operand),
            })
        }

        BoundExpression::Function(f) => {
            let args = f
                .args
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::function(f.name.clone(), args))
        }

        BoundExpression::Aggregate(a) => {
            let args = a
                .arguments
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            let func = function_name_to_aggregate(a)?;
            Ok(Expression::Aggregate {
                func,
                args,
                distinct: a.distinct,
                filter: None,
            })
        }

        BoundExpression::ParameterRef(name, _) => Ok(Expression::Parameter(name.clone())),

        BoundExpression::SessionVariable(name, _) => Ok(Expression::SessionVariable(name.clone())),

        BoundExpression::Cast {
            expr, target_type, ..
        } => {
            let e = convert_bound_to_expression(expr)?;
            Ok(Expression::TypeCast {
                expression: Box::new(e),
                target_type: target_type.clone(),
            })
        }

        BoundExpression::List(items, _) => {
            let items = items
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::List(items))
        }

        BoundExpression::Map(entries, _) => {
            let entries = entries
                .iter()
                .map(|(k, v)| convert_bound_to_expression(v).map(|e| (k.clone(), e)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::Map(entries))
        }

        BoundExpression::Case {
            expr,
            when_then,
            else_expr,
            ..
        } => {
            let test_expr = expr
                .as_ref()
                .map(|e| convert_bound_to_expression(e))
                .transpose()?;
            let conditions = when_then
                .iter()
                .map(|(when, then)| {
                    let w = convert_bound_to_expression(when)?;
                    let t = convert_bound_to_expression(then)?;
                    Ok::<_, String>((w, t))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let default = else_expr
                .as_ref()
                .map(|e| convert_bound_to_expression(e))
                .transpose()?;
            Ok(Expression::Case {
                test_expr: test_expr.map(Box::new),
                conditions,
                default: default.map(Box::new),
            })
        }

        BoundExpression::Label(s) => Ok(Expression::Label(s.clone())),

        BoundExpression::TagProperty {
            tag_name, property, ..
        } => Ok(Expression::TagProperty {
            tag_name: tag_name.clone(),
            property: property.clone(),
        }),

        BoundExpression::EdgeProperty {
            edge_name,
            property,
            ..
        } => Ok(Expression::EdgeProperty {
            edge_name: edge_name.clone(),
            property: property.clone(),
        }),

        BoundExpression::Predicate { func, args, .. } => {
            let args = args
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::Predicate {
                func: func.clone(),
                args,
            })
        }

        BoundExpression::Subscript {
            collection, index, ..
        } => {
            let collection = convert_bound_to_expression(collection)?;
            let index = convert_bound_to_expression(index)?;
            Ok(Expression::Subscript {
                collection: Box::new(collection),
                index: Box::new(index),
            })
        }

        BoundExpression::WindowFunction {
            name,
            args,
            over_partition_by,
            over_order_by,
            over_order_desc,
            ..
        } => {
            let args = args
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            let over_partition_by = over_partition_by
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            let over_order_by = over_order_by
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::WindowFunction {
                name: name.clone(),
                args,
                over_partition_by,
                over_order_by,
                over_order_desc: over_order_desc.clone(),
            })
        }

        BoundExpression::Path(items, _) => {
            let items = items
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::Path(items))
        }

        BoundExpression::ListComprehension {
            variable,
            source,
            filter,
            map,
            ..
        } => {
            let source = convert_bound_to_expression(source)?;
            let filter_exp = filter
                .as_ref()
                .map(|e| convert_bound_to_expression(e))
                .transpose()?;
            let map_exp = map
                .as_ref()
                .map(|e| convert_bound_to_expression(e))
                .transpose()?;
            Ok(Expression::ListComprehension {
                variable: variable.clone(),
                source: Box::new(source),
                filter: filter_exp.map(Box::new),
                map: map_exp.map(Box::new),
            })
        }

        BoundExpression::Reduce {
            accumulator,
            initial,
            variable,
            source,
            mapping,
            ..
        } => {
            let initial = convert_bound_to_expression(initial)?;
            let source = convert_bound_to_expression(source)?;
            let mapping = convert_bound_to_expression(mapping)?;
            Ok(Expression::Reduce {
                accumulator: accumulator.clone(),
                initial: Box::new(initial),
                variable: variable.clone(),
                source: Box::new(source),
                mapping: Box::new(mapping),
            })
        }

        BoundExpression::PathBuild(items, _) => {
            let items = items
                .iter()
                .map(convert_bound_to_expression)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Expression::PathBuild(items))
        }

        BoundExpression::Vector(v) => Ok(Expression::Vector(v.clone())),

        BoundExpression::Subquery { original_body, .. } => match original_body {
            Some(body) => Ok(Expression::scalar_subquery(body.as_ref().clone())),
            None => Err(
                "Subquery expression conversion requires original AST context".to_string(),
            ),
        },
        BoundExpression::Exists {
            query: _,
            original_body,
        } => match original_body {
            Some(body) => Ok(Expression::Exists { body: body.clone() }),
            None => Err("Exists expression conversion requires original AST context".to_string()),
        },
        BoundExpression::In {
            expr,
            subquery: _,
            negated,
            original_body,
        } => match original_body {
            Some(body) => {
                let inner = convert_bound_to_expression(expr)?;
                Ok(Expression::In {
                    expr: Box::new(inner),
                    subquery: body.clone(),
                    negated: *negated,
                })
            }
            None => Err("In expression conversion requires original AST context".to_string()),
        },
        BoundExpression::CountSubquery {
            query: _,
            original_body,
        } => match original_body {
            Some(body) => Ok(Expression::CountSubquery { body: body.clone() }),
            None => {
                Err("CountSubquery expression conversion requires original AST context".to_string())
            }
        },
        BoundExpression::Lambda { params, body } => {
            let body_expr = convert_bound_to_expression(body)?;
            Ok(Expression::Lambda {
                params: params.clone(),
                body: Box::new(body_expr),
            })
        }
        BoundExpression::Pattern(_) => {
            Err("Pattern expression conversion is intentionally unsupported without original AST context".to_string())
        }
    }
}

fn function_name_to_aggregate(a: &BoundAggregateCall) -> Result<AggregateFunction, String> {
    let name = &a.function_name;

    match name.to_uppercase().as_str() {
        "COUNT" => Ok(AggregateFunction::Count),
        "SUM" => Ok(AggregateFunction::Sum),
        "AVG" | "AVERAGE" => Ok(AggregateFunction::Avg),
        "MIN" => Ok(AggregateFunction::Min),
        "MAX" => Ok(AggregateFunction::Max),
        "COLLECT" => Ok(AggregateFunction::Collect),
        "COLLECT_SET" => Ok(AggregateFunction::CollectSet),
        "PERCENTILE" => Ok(AggregateFunction::Percentile),
        "PERCENTILE_CONT" => Ok(AggregateFunction::PercentileCont),
        "STD" | "STDDEV" => Ok(AggregateFunction::Std),
        "STDDEV_POP" => Ok(AggregateFunction::StddevPop),
        "STDDEV_SAMP" => Ok(AggregateFunction::StddevSamp),
        "VARIANCE" | "VAR" => Ok(AggregateFunction::Variance),
        "PRODUCT" => Ok(AggregateFunction::Product),
        "MEDIAN" => Ok(AggregateFunction::Median),
        "MODE" => Ok(AggregateFunction::Mode),
        "BIT_AND" => Ok(AggregateFunction::BitAnd),
        "BIT_OR" => Ok(AggregateFunction::BitOr),
        "BOOL_AND" => Ok(AggregateFunction::BoolAnd),
        "BOOL_OR" => Ok(AggregateFunction::BoolOr),
        "GROUP_CONCAT" => Ok(AggregateFunction::GroupConcat),
        "GROUP_CONCAT_WITH_ORDER" => Ok(AggregateFunction::GroupConcatWithOrder),
        "VEC_SUM" => Ok(AggregateFunction::VecSum),
        "VEC_AVG" => Ok(AggregateFunction::VecAvg),
        _ => Err(format!("Unknown aggregate function: {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::bound::BoundProjectionItem;
    use graphdb_core::DataType;

    #[test]
    fn test_bound_property_ref_preserves_type() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let bound = BoundExpression::Property {
            object: Box::new(BoundExpression::Variable(
                "n".to_string(),
                DataType::Unknown,
            )),
            property: "age".to_string(),
            value_type: DataType::Int,
        };
        let ctx_expr = bound_expr_to_contextual(&bound, &ctx).expect("convert");
        assert_eq!(ctx_expr.data_type(), Some(DataType::Int));
    }

    #[test]
    fn test_bound_projection_default_alias_and_type() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let item = BoundProjectionItem {
            expression: BoundExpression::Variable("n".to_string(), DataType::String),
            alias: None,
        };
        let col = bound_projection_to_yield_column(&item, &ctx).expect("convert");
        assert_eq!(col.alias, "n");
        assert_eq!(col.expression.data_type(), Some(DataType::String));
    }
}
