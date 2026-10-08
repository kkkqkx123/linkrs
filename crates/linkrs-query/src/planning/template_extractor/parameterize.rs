use super::ParameterizedResult;
use linkrs_core::types::expr::{ContextualExpression, Expression, FunctionArg};

/// Expression Parameterized Translator
///
/// Traverse the expression tree and replace all literals with parameter placeholders.
pub struct ParameterizingTransformer;

impl ParameterizingTransformer {
    pub fn new() -> Self {
        Self
    }

    /// Parameterizing a single ContextualExpression
    pub fn parameterize(&mut self, expr: &ContextualExpression) -> ParameterizedResult {
        let inner_expr = match expr.get_expression() {
            Some(e) => e,
            None => {
                return ParameterizedResult::new();
            }
        };
        let mut result = ParameterizedResult::new();
        let new_expr = self.transform_with_params(&inner_expr, &mut result);
        result.expression = new_expr;
        result
    }

    /// Parameterizing a single Expression (compatible with older interfaces)
    pub fn parameterize_expression(&mut self, expr: &Expression) -> ParameterizedResult {
        let mut result = ParameterizedResult::new();
        let new_expr = self.transform_with_params(expr, &mut result);
        result.expression = new_expr;
        result
    }

    /// Parameterizing multiple expressions
    pub fn parameterize_many(
        &mut self,
        exprs: &[ContextualExpression],
    ) -> Vec<ParameterizedResult> {
        exprs.iter().map(|expr| self.parameterize(expr)).collect()
    }

    fn transform_with_params(
        &mut self,
        expr: &Expression,
        result: &mut ParameterizedResult,
    ) -> Expression {
        match expr {
            Expression::Literal(value) => {
                let param_name = result.add_parameter(value.clone());
                Expression::Variable(param_name)
            }
            Expression::Binary { left, op, right } => {
                let new_left = self.transform_with_params(left, result);
                let new_right = self.transform_with_params(right, result);
                Expression::Binary {
                    left: Box::new(new_left),
                    op: *op,
                    right: Box::new(new_right),
                }
            }
            Expression::Unary { op, operand } => {
                let new_operand = self.transform_with_params(operand, result);
                Expression::Unary {
                    op: *op,
                    operand: Box::new(new_operand),
                }
            }
            Expression::Function { name, args } => {
                let new_args: Vec<FunctionArg> = args
                    .iter()
                    .map(|arg| {
                        FunctionArg::Positional(self.transform_with_params(arg.as_expr(), result))
                    })
                    .collect();
                Expression::Function {
                    name: name.clone(),
                    args: new_args,
                }
            }
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter,
            } => {
                let new_args: Vec<Expression> = args
                    .iter()
                    .map(|a| self.transform_with_params(a, result))
                    .collect();
                let new_filter = filter
                    .as_ref()
                    .map(|f| self.transform_with_params(f, result));
                Expression::Aggregate {
                    func: *func,
                    args: new_args,
                    distinct: *distinct,
                    filter: new_filter.map(Box::new),
                }
            }
            Expression::List(items) => {
                let new_items: Vec<Expression> = items
                    .iter()
                    .map(|item| self.transform_with_params(item, result))
                    .collect();
                Expression::List(new_items)
            }
            Expression::Map(pairs) => {
                let new_pairs: Vec<(String, Expression)> = pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), self.transform_with_params(v, result)))
                    .collect();
                Expression::Map(new_pairs)
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let new_test_expr = test_expr
                    .as_ref()
                    .map(|e| Box::new(self.transform_with_params(e, result)));
                let new_conditions: Vec<(Expression, Expression)> = conditions
                    .iter()
                    .map(|(cond, val)| {
                        (
                            self.transform_with_params(cond, result),
                            self.transform_with_params(val, result),
                        )
                    })
                    .collect();
                let new_default = default
                    .as_ref()
                    .map(|d| Box::new(self.transform_with_params(d, result)));
                Expression::Case {
                    test_expr: new_test_expr,
                    conditions: new_conditions,
                    default: new_default,
                }
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let new_expr = self.transform_with_params(expression, result);
                Expression::TypeCast {
                    expression: Box::new(new_expr),
                    target_type: target_type.clone(),
                }
            }
            Expression::Subscript { collection, index } => {
                let new_collection = self.transform_with_params(collection, result);
                let new_index = self.transform_with_params(index, result);
                Expression::Subscript {
                    collection: Box::new(new_collection),
                    index: Box::new(new_index),
                }
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let new_collection = self.transform_with_params(collection, result);
                let new_start = start
                    .as_ref()
                    .map(|s| Box::new(self.transform_with_params(s, result)));
                let new_end = end
                    .as_ref()
                    .map(|e| Box::new(self.transform_with_params(e, result)));
                Expression::Range {
                    collection: Box::new(new_collection),
                    start: new_start,
                    end: new_end,
                }
            }
            Expression::Path(items) => {
                let new_items: Vec<Expression> = items
                    .iter()
                    .map(|item| self.transform_with_params(item, result))
                    .collect();
                Expression::Path(new_items)
            }
            Expression::Property { object, property } => {
                let new_object = self.transform_with_params(object, result);
                Expression::Property {
                    object: Box::new(new_object),
                    property: property.clone(),
                }
            }
            Expression::StructField { base, field } => {
                let new_base = self.transform_with_params(base, result);
                Expression::StructField {
                    base: Box::new(new_base),
                    field: field.clone(),
                }
            }
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let new_source = self.transform_with_params(source, result);
                let new_filter = filter
                    .as_ref()
                    .map(|f| Box::new(self.transform_with_params(f, result)));
                let new_map = map
                    .as_ref()
                    .map(|m| Box::new(self.transform_with_params(m, result)));
                Expression::ListComprehension {
                    variable: variable.clone(),
                    source: Box::new(new_source),
                    filter: new_filter,
                    map: new_map,
                }
            }
            Expression::LabelTagProperty { tag, property } => {
                let new_tag = self.transform_with_params(tag, result);
                Expression::LabelTagProperty {
                    tag: Box::new(new_tag),
                    property: property.clone(),
                }
            }
            Expression::Predicate { func, args } => {
                let new_args: Vec<Expression> = args
                    .iter()
                    .map(|arg| self.transform_with_params(arg, result))
                    .collect();
                Expression::Predicate {
                    func: func.clone(),
                    args: new_args,
                }
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                let new_initial = self.transform_with_params(initial, result);
                let new_source = self.transform_with_params(source, result);
                let new_mapping = self.transform_with_params(mapping, result);
                Expression::Reduce {
                    accumulator: accumulator.clone(),
                    initial: Box::new(new_initial),
                    variable: variable.clone(),
                    source: Box::new(new_source),
                    mapping: Box::new(new_mapping),
                }
            }
            Expression::PathBuild(exprs) => {
                let new_exprs: Vec<Expression> = exprs
                    .iter()
                    .map(|e| self.transform_with_params(e, result))
                    .collect();
                Expression::PathBuild(new_exprs)
            }
            Expression::Variable(_)
            | Expression::Label(_)
            | Expression::TagProperty { .. }
            | Expression::EdgeProperty { .. }
            | Expression::Parameter(_)
            | Expression::SessionVariable(_)
            | Expression::Vector(_)
            | Expression::Exists { .. }
            | Expression::In { .. }
            | Expression::WindowFunction { .. } => expr.clone(),
            Expression::CountSubquery { body } => {
                Expression::count_subquery(self.transform_subquery_body(body, result))
            }
            Expression::ScalarSubquery { body } => {
                Expression::scalar_subquery(self.transform_subquery_body(body, result))
            }
            Expression::Lambda { params, body } => {
                let transformed_body = self.transform_with_params(body, result);
                Expression::Lambda {
                    params: params.clone(),
                    body: Box::new(transformed_body),
                }
            }
        }
    }

    fn transform_subquery_body(
        &mut self,
        body: &linkrs_core::types::expr::SubqueryBody,
        result: &mut ParameterizedResult,
    ) -> linkrs_core::types::expr::SubqueryBody {
        let where_clause = body.where_clause.as_ref().map(|expr| {
            let transformed = self.transform_with_params(expr, result);
            Box::new(transformed)
        });
        let return_expr = body.return_expr.as_ref().map(|expr| {
            let transformed = self.transform_with_params(expr, result);
            Box::new(transformed)
        });
        linkrs_core::types::expr::SubqueryBody {
            id: body.id,
            patterns: body.patterns.clone(),
            where_clause,
            return_expr,
        }
    }
}

impl Default for ParameterizingTransformer {
    fn default() -> Self {
        Self::new()
    }
}
