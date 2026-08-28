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
    Ok(ContextualExpression::new(id, ctx.clone()))
}

fn convert_bound_to_expression(bound: &BoundExpression) -> Result<Expression, String> {
    match bound {
        BoundExpression::Literal(v, _) => Ok(Expression::Literal(v.clone())),

        BoundExpression::Variable(name, _) => Ok(Expression::Variable(name.clone())),

        BoundExpression::ColumnRef(cr) => Ok(Expression::Property {
            object: Box::new(Expression::Variable(cr.variable.clone())),
            property: cr.property.clone(),
        }),

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
            Ok(Expression::Function {
                name: f.name.clone(),
                args,
            })
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

        BoundExpression::Subquery(_) => {
            Err("Subquery expression conversion not yet supported".to_string())
        }
        BoundExpression::Exists { .. } => {
            Err("Exists expression conversion not yet supported".to_string())
        }
        BoundExpression::In { .. } => Err("In expression conversion not yet supported".to_string()),
        BoundExpression::Pattern(_) => {
            Err("Pattern expression conversion not yet supported".to_string())
        }
    }
}

fn function_name_to_aggregate(a: &BoundAggregateCall) -> Result<AggregateFunction, String> {
    let name = &a.function_name;

    match name.to_uppercase().as_str() {
        "COUNT" => Ok(AggregateFunction::Count),
        "SUM" => Ok(AggregateFunction::Sum),
        "AVG" => Ok(AggregateFunction::Avg),
        "MIN" => Ok(AggregateFunction::Min),
        "MAX" => Ok(AggregateFunction::Max),
        "COLLECT" => Ok(AggregateFunction::Collect),
        "STD" => Ok(AggregateFunction::Std),
        "VARIANCE" => Ok(AggregateFunction::Variance),
        "PRODUCT" => Ok(AggregateFunction::Product),
        "MEDIAN" => Ok(AggregateFunction::Median),
        "MODE" => Ok(AggregateFunction::Mode),
        _ => Err(format!("Unknown aggregate function: {}", name)),
    }
}
