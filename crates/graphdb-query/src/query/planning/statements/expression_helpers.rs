use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::operators::BinaryOperator;
use crate::core::types::ContextualExpression;
use crate::core::Expression;
use std::sync::Arc;

pub fn build_label_filter_expression(
    variable: &Option<String>,
    labels: &[String],
    expr_context: &Arc<ExpressionAnalysisContext>,
) -> ContextualExpression {
    let var_name = variable.as_deref().unwrap_or("n");
    let var_expr = Expression::variable(var_name);

    let ctx = expr_context.clone();

    let labels_func = Expression::function("labels", vec![var_expr]);

    let expr = if labels.len() == 1 {
        let label_literal = Expression::literal(labels[0].clone());
        Expression::function("contains", vec![labels_func, label_literal])
    } else {
        let first_label = Expression::literal(labels[0].clone());
        let first_condition =
            Expression::function("contains", vec![labels_func.clone(), first_label]);

        labels.iter().skip(1).fold(first_condition, |acc, label| {
            let label_literal = Expression::literal(label.clone());
            let condition =
                Expression::function("contains", vec![labels_func.clone(), label_literal]);
            Expression::binary(acc, BinaryOperator::And, condition)
        })
    };

    let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    ContextualExpression::new(id, ctx)
}

pub fn convert_properties_to_filter(
    var_name: &str,
    props: &ContextualExpression,
    expr_context: &Arc<ExpressionAnalysisContext>,
) -> Option<ContextualExpression> {
    let props_expr = props.expression()?.inner().clone();

    if let Expression::Map(pairs) = props_expr {
        if pairs.is_empty() {
            return None;
        }

        let var_expr = Expression::variable(var_name);

        let conditions: Vec<Expression> = pairs
            .into_iter()
            .map(|(key, value)| {
                let prop_access = Expression::property(var_expr.clone(), key);
                Expression::binary(prop_access, BinaryOperator::Equal, value)
            })
            .collect();

        let combined = conditions
            .into_iter()
            .reduce(|acc, cond| Expression::binary(acc, BinaryOperator::And, cond))?;

        let expr_meta = crate::core::types::expr::ExpressionMeta::new(combined);
        let id = expr_context.register_expression(expr_meta);
        Some(ContextualExpression::new(id, expr_context.clone()))
    } else {
        None
    }
}
