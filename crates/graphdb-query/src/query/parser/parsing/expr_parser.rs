//! Expression parsing module
//!
//! Responsible for parsing various expressions, including arithmetic expressions, logical expressions, function calls, etc.
//! Generate the Core Expression directly, avoiding the redundant conversion to AST Expression.

use std::sync::Arc;

use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta, SubqueryBody};
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::types::{DataType, Position, Span};
use crate::core::Value;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Expression analysis results, including the expression itself and information about its location.
pub struct ParseResult {
    pub expr: Expression,
    pub span: Span,
}

pub struct ExprParser<'a> {
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> ExprParser<'a> {
    pub fn new(_ctx: &ParseContext<'a>) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn parse_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        self.parse_or_expression(ctx)
    }

    /// Parse the expression and return the ContextualExpression.
    pub fn parse_expression_with_context(
        &mut self,
        ctx: &mut ParseContext<'a>,
        expr_ctx: Arc<ExpressionAnalysisContext>,
    ) -> Result<ContextualExpression, ParseError> {
        let result = self.parse_expression(ctx)?;
        let expr_meta = ExpressionMeta::with_span(result.expr, result.span);
        let id = expr_ctx.register_expression(expr_meta);
        Ok(ContextualExpression::new(id, expr_ctx))
    }

    fn parse_or_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut left = self.parse_and_expression(ctx)?;

        while ctx.match_token(TokenKind::Or) {
            let op = BinaryOperator::Or;
            let right = self.parse_and_expression(ctx)?;
            let span = ctx.merge_span(left.span.start, right.span.end);
            left = ParseResult {
                expr: Expression::binary(left.expr, op, right.expr),
                span,
            };
        }

        Ok(left)
    }

    fn parse_and_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut left = self.parse_not_expression(ctx)?;

        while ctx.match_token(TokenKind::And) {
            let op = BinaryOperator::And;
            let right = self.parse_not_expression(ctx)?;
            let span = ctx.merge_span(left.span.start, right.span.end);
            left = ParseResult {
                expr: Expression::binary(left.expr, op, right.expr),
                span,
            };
        }

        Ok(left)
    }

    fn parse_not_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        if ctx.match_token(TokenKind::Not) {
            let op = UnaryOperator::Not;
            let operand = self.parse_not_expression(ctx)?;
            let span = ctx.merge_span(operand.span.start, operand.span.end);
            Ok(ParseResult {
                expr: Expression::unary(op, operand.expr),
                span,
            })
        } else {
            self.parse_comparison_expression(ctx)
        }
    }

    fn parse_comparison_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut left = self.parse_additive_expression(ctx)?;

        if let Some(op) = self.parse_comparison_op(ctx) {
            let right = self.parse_additive_expression(ctx)?;
            let span = ctx.merge_span(left.span.start, right.span.end);
            left = ParseResult {
                expr: Expression::binary(left.expr, op, right.expr),
                span,
            };
        }

        Ok(left)
    }

    fn parse_comparison_op(&mut self, ctx: &mut ParseContext<'a>) -> Option<BinaryOperator> {
        match ctx.current_token().kind {
            TokenKind::Eq => {
                ctx.next_token();
                Some(BinaryOperator::Equal)
            }
            TokenKind::Ne => {
                ctx.next_token();
                Some(BinaryOperator::NotEqual)
            }
            TokenKind::Lt => {
                ctx.next_token();
                Some(BinaryOperator::LessThan)
            }
            TokenKind::Le => {
                ctx.next_token();
                Some(BinaryOperator::LessThanOrEqual)
            }
            TokenKind::Gt => {
                ctx.next_token();
                Some(BinaryOperator::GreaterThan)
            }
            TokenKind::Ge => {
                ctx.next_token();
                Some(BinaryOperator::GreaterThanOrEqual)
            }
            TokenKind::Regex => {
                ctx.next_token();
                Some(BinaryOperator::Like)
            }
            TokenKind::Contains => {
                ctx.next_token();
                Some(BinaryOperator::Contains)
            }
            TokenKind::StartsWith => {
                ctx.next_token();
                // The consumption of the optional WITH token is allowed (STARTS WITH is a keyword that consists of two words).
                ctx.match_token(TokenKind::With);
                Some(BinaryOperator::StartsWith)
            }
            TokenKind::EndsWith => {
                ctx.next_token();
                // The consumption of the optional WITH token is allowed (ENDS WITH is a keyword consisting of two words).
                ctx.match_token(TokenKind::With);
                Some(BinaryOperator::EndsWith)
            }
            _ => None,
        }
    }

    fn parse_additive_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut left = self.parse_multiplicative_expression(ctx)?;

        while let Some(op) = self.parse_additive_op(ctx) {
            let right = self.parse_multiplicative_expression(ctx)?;
            let span = ctx.merge_span(left.span.start, right.span.end);
            left = ParseResult {
                expr: Expression::binary(left.expr, op, right.expr),
                span,
            };
        }

        Ok(left)
    }

    fn parse_additive_op(&mut self, ctx: &mut ParseContext<'a>) -> Option<BinaryOperator> {
        match ctx.current_token().kind {
            TokenKind::Plus => {
                ctx.next_token();
                Some(BinaryOperator::Add)
            }
            TokenKind::Minus => {
                ctx.next_token();
                Some(BinaryOperator::Subtract)
            }
            _ => None,
        }
    }

    fn parse_multiplicative_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut left = self.parse_unary_expression(ctx)?;

        while let Some(op) = self.parse_multiplicative_op(ctx) {
            let right = self.parse_unary_expression(ctx)?;
            let span = ctx.merge_span(left.span.start, right.span.end);
            left = ParseResult {
                expr: Expression::binary(left.expr, op, right.expr),
                span,
            };
        }

        Ok(left)
    }

    fn parse_multiplicative_op(&mut self, ctx: &mut ParseContext<'a>) -> Option<BinaryOperator> {
        match ctx.current_token().kind {
            TokenKind::Star => {
                ctx.next_token();
                Some(BinaryOperator::Multiply)
            }
            TokenKind::Div => {
                ctx.next_token();
                Some(BinaryOperator::Divide)
            }
            TokenKind::Mod => {
                ctx.next_token();
                Some(BinaryOperator::Modulo)
            }
            _ => None,
        }
    }

    fn parse_unary_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        if ctx.match_token(TokenKind::Minus) {
            let op = UnaryOperator::Minus;
            let operand = self.parse_unary_expression(ctx)?;
            let span = ctx.merge_span(operand.span.start, operand.span.end);
            Ok(ParseResult {
                expr: Expression::unary(op, operand.expr),
                span,
            })
        } else if ctx.match_token(TokenKind::Plus) {
            let op = UnaryOperator::Plus;
            let operand = self.parse_unary_expression(ctx)?;
            let span = ctx.merge_span(operand.span.start, operand.span.end);
            Ok(ParseResult {
                expr: Expression::unary(op, operand.expr),
                span,
            })
        } else if ctx.match_token(TokenKind::NotOp) {
            let op = UnaryOperator::Not;
            let operand = self.parse_unary_expression(ctx)?;
            let span = ctx.merge_span(operand.span.start, operand.span.end);
            Ok(ParseResult {
                expr: Expression::unary(op, operand.expr),
                span,
            })
        } else {
            self.parse_exponentiation_expression(ctx)
        }
    }

    fn parse_exponentiation_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut expression = self.parse_postfix_expression(ctx)?;

        if ctx.match_token(TokenKind::Exp) {
            let mut right_operands = Vec::new();

            while ctx.match_token(TokenKind::Exp) {
                right_operands.push(self.parse_unary_expression(ctx)?);
            }

            for operand in right_operands.into_iter().rev() {
                let span = ctx.merge_span(expression.span.start, operand.span.end);
                expression = ParseResult {
                    expr: Expression::binary(
                        expression.expr,
                        BinaryOperator::Exponent,
                        operand.expr,
                    ),
                    span,
                };
            }
        }

        Ok(expression)
    }

    fn parse_postfix_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let mut expression = self.parse_primary_expression(ctx)?;

        loop {
            if ctx.match_token(TokenKind::LBracket) {
                let index = self.parse_expression(ctx)?;
                ctx.expect_token(TokenKind::RBracket)?;
                let span = ctx.merge_span(expression.span.start, ctx.current_position());
                expression = ParseResult {
                    expr: Expression::subscript(expression.expr, index.expr),
                    span,
                };
            } else if ctx.match_token(TokenKind::Dot) {
                let property = ctx.expect_identifier()?;
                let span = ctx.merge_span(expression.span.start, ctx.current_position());
                expression = ParseResult {
                    expr: Expression::property(expression.expr, property),
                    span,
                };
            } else if ctx.match_token(TokenKind::DoubleColon) {
                // Type cast syntax: expr::TYPE
                let type_name = ctx.expect_identifier()?;
                let span = ctx.merge_span(expression.span.start, ctx.current_position());

                // Check if casting to VECTOR
                if type_name.to_uppercase() == "VECTOR" {
                    // Convert list expression to vector
                    if let Expression::List(elements) = expression.expr.clone() {
                        let mut vector_data = Vec::with_capacity(elements.len());
                        for elem in elements {
                            if let Expression::Literal(Value::Double(f)) = elem {
                                vector_data.push(f as f32);
                            } else if let Expression::Literal(Value::Float(f)) = elem {
                                vector_data.push(f);
                            } else if let Expression::Literal(Value::Int(i)) = elem {
                                vector_data.push(i as f32);
                            } else if let Expression::Literal(Value::BigInt(i)) = elem {
                                vector_data.push(i as f32);
                            } else {
                                return Err(ParseError::new(
                                    ParseErrorKind::SemanticError,
                                    "Vector elements must be numeric literals".to_string(),
                                    span.start,
                                ));
                            }
                        }
                        expression = ParseResult {
                            expr: Expression::vector(vector_data),
                            span,
                        };
                    } else {
                        return Err(ParseError::new(
                            ParseErrorKind::SemanticError,
                            "Can only cast list literals to VECTOR".to_string(),
                            span.start,
                        ));
                    }
                } else {
                    let target_type = match type_name.to_uppercase().as_str() {
                        "BOOL" | "BOOLEAN" => DataType::Bool,
                        "INT" | "INTEGER" | "INT4" => DataType::Int,
                        "BIGINT" | "INT8" => DataType::BigInt,
                        "SMALLINT" | "INT2" => DataType::SmallInt,
                        "FLOAT" | "FLOAT4" => DataType::Float,
                        "DOUBLE" | "FLOAT8" | "DOUBLE PRECISION" => DataType::Double,
                        "STRING" | "TEXT" | "VARCHAR" => DataType::String,
                        "DATE" => DataType::Date,
                        "TIME" => DataType::Time,
                        "DATETIME" => DataType::DateTime,
                        "TIMESTAMP" => DataType::Timestamp,
                        "LIST" => DataType::List,
                        "MAP" => DataType::Map,
                        "SET" => DataType::Set,
                        "JSON" => DataType::Json,
                        "JSONB" => DataType::JsonB,
                        "UUID" => DataType::Uuid,
                        "INTERVAL" => DataType::Interval,
                        "BLOB" => DataType::Blob,
                        "GEOGRAPHY" => DataType::Geography,
                        _ => {
                            return Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                format!("Unknown type cast target: {}", type_name),
                                span.start,
                            ));
                        }
                    };
                    expression = ParseResult {
                        expr: Expression::TypeCast {
                            expression: Box::new(expression.expr),
                            target_type,
                        },
                        span,
                    };
                }
            } else if ctx.check_token(TokenKind::In) && ctx.peek_token().kind == TokenKind::LBrace {
                // IN subquery: expr IN { MATCH ... }
                ctx.match_token(TokenKind::In);
                ctx.expect_token(TokenKind::LBrace)?;
                let subquery = self.parse_subquery_body(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                let span = ctx.merge_span(expression.span.start, ctx.current_position());
                expression = ParseResult {
                    expr: Expression::in_subquery(expression.expr, subquery, false),
                    span,
                };
            } else {
                break;
            }
        }

        Ok(expression)
    }

    fn parse_primary_expression(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let token = ctx.current_token().clone();
        let start_pos = ctx.current_position();

        match token.kind {
            TokenKind::LParen => {
                ctx.next_token();
                let expression = self.parse_expression(ctx)?;
                ctx.expect_token(TokenKind::RParen)?;
                Ok(expression)
            }
            TokenKind::Identifier(name) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                if ctx.match_token(TokenKind::LParen) {
                    self.parse_function_call(name, span, ctx)
                } else {
                    Ok(ParseResult {
                        expr: Expression::variable(name),
                        span,
                    })
                }
            }
            // Allow certain keywords to be used as variable names in expressions
            TokenKind::Edge => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                let mut expr = Expression::variable("edge".to_string());
                if ctx.match_token(TokenKind::Dot) {
                    let prop_name = ctx.expect_identifier()?;
                    expr = Expression::property(expr, prop_name);
                }
                Ok(ParseResult { expr, span })
            }
            TokenKind::IntegerLiteral(n) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::literal(Value::BigInt(n)),
                    span,
                })
            }
            TokenKind::FloatLiteral(f) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::literal(Value::Double(f)),
                    span,
                })
            }
            TokenKind::StringLiteral(s) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::literal(Value::String(s)),
                    span,
                })
            }
            TokenKind::BooleanLiteral(b) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::literal(Value::Bool(b)),
                    span,
                })
            }
            TokenKind::Null => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::literal(Value::Null(crate::core::NullType::Null)),
                    span,
                })
            }
            TokenKind::Count
            | TokenKind::Sum
            | TokenKind::Avg
            | TokenKind::Min
            | TokenKind::Max => {
                let func_name = token.lexeme.clone();
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                if ctx.match_token(TokenKind::LParen) {
                    self.parse_function_call(func_name, span, ctx)
                } else {
                    Ok(ParseResult {
                        expr: Expression::variable(func_name),
                        span,
                    })
                }
            }
            TokenKind::User
            | TokenKind::Order
            | TokenKind::Status
            | TokenKind::Contains
            | TokenKind::Tag
            | TokenKind::Tags
            | TokenKind::Path
            | TokenKind::Vertex
            | TokenKind::Vertices
            | TokenKind::Edges => {
                let name = token.lexeme.clone();
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                if ctx.match_token(TokenKind::LParen) {
                    self.parse_function_call(name, span, ctx)
                } else {
                    Ok(ParseResult {
                        expr: Expression::variable(name),
                        span,
                    })
                }
            }
            TokenKind::List => {
                ctx.next_token();
                let elements = self.parse_expression_list(ctx)?;
                ctx.expect_token(TokenKind::RBracket)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::list(elements.into_iter().map(|e| e.expr).collect()),
                    span,
                })
            }
            TokenKind::LBracket => {
                ctx.next_token();
                if ctx.is_identifier_or_in_token() {
                    self.parse_list_comprehension(start_pos, ctx)
                } else if ctx.match_token(TokenKind::RBracket) {
                    let span = ctx.merge_span(start_pos, ctx.current_position());
                    Ok(ParseResult {
                        expr: Expression::list(Vec::new()),
                        span,
                    })
                } else {
                    let elements = self.parse_expression_list(ctx)?;
                    ctx.expect_token(TokenKind::RBracket)?;
                    let span = ctx.merge_span(start_pos, ctx.current_position());
                    Ok(ParseResult {
                        expr: Expression::list(elements.into_iter().map(|e| e.expr).collect()),
                        span,
                    })
                }
            }
            TokenKind::Case => self.parse_case_expression(start_pos, ctx),
            TokenKind::Map => {
                ctx.next_token();
                ctx.expect_token(TokenKind::LBrace)?;
                let properties = self.parse_property_list(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::map(
                        properties.into_iter().map(|(k, v)| (k, v.expr)).collect(),
                    ),
                    span,
                })
            }
            TokenKind::LBrace => {
                ctx.next_token();
                let properties = self.parse_property_list(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::map(
                        properties.into_iter().map(|(k, v)| (k, v.expr)).collect(),
                    ),
                    span,
                })
            }
            TokenKind::InputRef => {
                ctx.next_token();
                let mut span = ctx.merge_span(start_pos, ctx.current_position());
                let mut expr = Expression::variable("$-");

                if ctx.match_token(TokenKind::Dot) {
                    let prop_name = ctx.expect_identifier()?;
                    expr = Expression::property(expr, prop_name);
                    // Update the span to include attribute access.
                    span = ctx.merge_span(start_pos, ctx.current_position());
                }

                Ok(ParseResult { expr, span })
            }
            TokenKind::SrcRef => {
                ctx.next_token();
                let mut span = ctx.merge_span(start_pos, ctx.current_position());
                let mut expr = Expression::variable("$^");

                if ctx.match_token(TokenKind::Dot) {
                    let prop_name = ctx.expect_identifier()?;
                    expr = Expression::property(expr, prop_name);
                    // Update the span to include attribute access.
                    span = ctx.merge_span(start_pos, ctx.current_position());
                }

                Ok(ParseResult { expr, span })
            }
            TokenKind::DstRef => {
                ctx.next_token();
                let mut span = ctx.merge_span(start_pos, ctx.current_position());
                let mut expr = Expression::variable("$$");

                if ctx.match_token(TokenKind::Dot) {
                    let prop_name = ctx.expect_identifier()?;
                    expr = Expression::property(expr, prop_name);
                    // Update the span to include attribute access.
                    span = ctx.merge_span(start_pos, ctx.current_position());
                }

                Ok(ParseResult { expr, span })
            }
            TokenKind::Dollar => {
                ctx.next_token();
                let var_name = ctx.expect_identifier()?;
                let mut span = ctx.merge_span(start_pos, ctx.current_position());
                let mut expr = Expression::variable(format!("${}", var_name));

                if ctx.match_token(TokenKind::Dot) {
                    let prop_name = ctx.expect_identifier()?;
                    expr = Expression::property(expr, prop_name);
                    // Update the `<span>` element to include access to the attributes.
                    span = ctx.merge_span(start_pos, ctx.current_position());
                }

                Ok(ParseResult { expr, span })
            }
            TokenKind::VectorLiteral(data) => {
                ctx.next_token();
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::vector(data),
                    span,
                })
            }
            TokenKind::Exists => {
                // EXISTS { pattern } or EXISTS(pattern)
                ctx.next_token();
                ctx.expect_token(TokenKind::LBrace)?;
                let body = self.parse_subquery_body(ctx)?;
                ctx.expect_token(TokenKind::RBrace)?;
                let span = ctx.merge_span(start_pos, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::exists(body),
                    span,
                })
            }
            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Unexpected token in expression: {:?}", token.kind),
                start_pos,
            )),
        }
    }

    fn parse_function_call(
        &mut self,
        name: String,
        span: Span,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let name_upper = name.to_uppercase();

        if ctx.match_token(TokenKind::Star) {
            ctx.expect_token(TokenKind::RParen)?;

            if name_upper == "COUNT" {
                return Ok(ParseResult {
                    expr: Expression::Aggregate {
                        func: crate::core::types::operators::AggregateFunction::Count(None),
                        args: vec![Expression::Literal(crate::core::Value::String(
                            "*".to_string(),
                        ))],
                        distinct: false,
                        filter: None,
                    },
                    span,
                });
            } else {
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    format!("Could not apply aggregation function `{}` on `*`", name),
                    ctx.current_position(),
                ));
            }
        }

        let args = if ctx.match_token(TokenKind::RParen) {
            Vec::new()
        } else {
            let args = self.parse_expression_list(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            args
        };

        let is_aggregate = matches!(
            name_upper.as_str(),
            "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COLLECT" | "COLLECT_SET"
                | "STD" | "STDDEV_POP" | "STDDEV_SAMP"
                | "PRODUCT" | "PERCENTILE_CONT"
                | "VARIANCE" | "MEDIAN" | "MODE"
                | "BOOL_AND" | "BOOL_OR" | "PERCENTILE"
                | "DISTINCT" | "BIT_AND" | "BIT_OR"
                | "GROUP_CONCAT" | "VEC_SUM" | "VEC_AVG"
        );

        if is_aggregate {
            let distinct = ctx.match_token(TokenKind::Distinct);
            let arg = args.first().map(|a| a.expr.clone()).unwrap_or_else(|| {
                Expression::Literal(crate::core::Value::Null(crate::core::NullType::Null))
            });

            let field_name = match &arg {
                Expression::Variable(name) => name.clone(),
                Expression::Property { object, property } => {
                    if let Expression::Variable(var_name) = object.as_ref() {
                        format!("{}.{}", var_name, property)
                    } else {
                        property.clone()
                    }
                }
                Expression::TagProperty { tag_name, property } => {
                    format!("{}.{}", tag_name, property)
                }
                Expression::EdgeProperty {
                    edge_name,
                    property,
                } => {
                    format!("{}.{}", edge_name, property)
                }
                _ => "_value".to_string(),
            };

            let func = match name_upper.as_str() {
                "COUNT" => {
                    crate::core::types::operators::AggregateFunction::Count(Some(field_name))
                }
                "SUM" => crate::core::types::operators::AggregateFunction::Sum(field_name),
                "AVG" => crate::core::types::operators::AggregateFunction::Avg(field_name),
                "MIN" => crate::core::types::operators::AggregateFunction::Min(field_name),
                "MAX" => crate::core::types::operators::AggregateFunction::Max(field_name),
                "COLLECT" => crate::core::types::operators::AggregateFunction::Collect(field_name),
                "COLLECT_SET" => {
                    crate::core::types::operators::AggregateFunction::CollectSet(field_name)
                }
                "STD" => crate::core::types::operators::AggregateFunction::Std(field_name),
                "STDDEV_POP" => crate::core::types::operators::AggregateFunction::StddevPop(field_name),
                "STDDEV_SAMP" => crate::core::types::operators::AggregateFunction::StddevSamp(field_name),
                "PRODUCT" => crate::core::types::operators::AggregateFunction::Product(field_name),
                "PERCENTILE_CONT" => {
                    let percentile = if args.len() > 1 {
                        match &args[1].expr {
                            Expression::Literal(crate::core::Value::Int(v)) => *v as f64,
                            Expression::Literal(crate::core::Value::BigInt(v)) => *v as f64,
                            Expression::Literal(crate::core::Value::Float(v)) => *v as f64,
                            Expression::Literal(crate::core::Value::Double(v)) => *v,
                            _ => 50.0,
                        }
                    } else {
                        50.0
                    };
                    crate::core::types::operators::AggregateFunction::PercentileCont(field_name, percentile)
                }
                "VARIANCE" => {
                    crate::core::types::operators::AggregateFunction::Variance(field_name)
                }
                "MEDIAN" => crate::core::types::operators::AggregateFunction::Median(field_name),
                "MODE" => crate::core::types::operators::AggregateFunction::Mode(field_name),
                "BOOL_AND" => {
                    crate::core::types::operators::AggregateFunction::BoolAnd(field_name)
                }
                "BOOL_OR" => crate::core::types::operators::AggregateFunction::BoolOr(field_name),
                _ => crate::core::types::operators::AggregateFunction::Count(None),
            };

            let filter = if ctx.match_token(TokenKind::Filter) {
                ctx.expect_token(TokenKind::LParen)?;
                ctx.expect_token(TokenKind::Where)?;
                let filter_expr = self.parse_expression(ctx)?;
                ctx.expect_token(TokenKind::RParen)?;
                Some(Box::new(filter_expr.expr))
            } else {
                None
            };

            let span = ctx.merge_span(span.start, ctx.current_position());
            Ok(ParseResult {
                expr: Expression::Aggregate {
                    func,
                    args: vec![arg],
                    distinct,
                    filter,
                },
                span,
            })
        } else {
            let func_args: Vec<Expression> = args.into_iter().map(|e| e.expr).collect();
            // Check for OVER clause (window function)
            if ctx.match_token(TokenKind::Over) {
                ctx.expect_token(TokenKind::LParen)?;
                let mut partition_by = Vec::new();
                let mut order_by = Vec::new();
                let mut order_desc = Vec::new();

                // Parse optional PARTITION BY clause
                if self.match_identifier_token(ctx, "PARTITION") {
                    ctx.expect_token(TokenKind::By)?;
                    partition_by.push(self.parse_expression(ctx)?.expr);
                    while ctx.match_token(TokenKind::Comma) {
                        partition_by.push(self.parse_expression(ctx)?.expr);
                    }
                }

                // Parse optional ORDER BY clause
                if ctx.match_token(TokenKind::Order) {
                    ctx.expect_token(TokenKind::By)?;
                    let first_expr = self.parse_expression(ctx)?;
                    let desc = if ctx.match_token(TokenKind::Desc) {
                        true
                    } else {
                        ctx.match_token(TokenKind::Asc);
                        false
                    };
                    order_by.push(first_expr.expr);
                    order_desc.push(desc);
                    while ctx.match_token(TokenKind::Comma) {
                        let expr = self.parse_expression(ctx)?.expr;
                        let d = if ctx.match_token(TokenKind::Desc) {
                            true
                        } else {
                            ctx.match_token(TokenKind::Asc);
                            false
                        };
                        order_by.push(expr);
                        order_desc.push(d);
                    }
                }

                ctx.expect_token(TokenKind::RParen)?;
                let span = ctx.merge_span(span.start, ctx.current_position());
                Ok(ParseResult {
                    expr: Expression::WindowFunction {
                        name,
                        args: func_args,
                        over_partition_by: partition_by,
                        over_order_by: order_by,
                        over_order_desc: order_desc,
                    },
                    span,
                })
            } else {
                Ok(ParseResult {
                    expr: Expression::Function {
                        name,
                        args: func_args,
                    },
                    span,
                })
            }
        }
    }

    fn parse_expression_list(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<Vec<ParseResult>, ParseError> {
        let mut expressions = Vec::new();
        expressions.push(self.parse_expression(ctx)?);
        while ctx.match_token(TokenKind::Comma) {
            expressions.push(self.parse_expression(ctx)?);
        }
        Ok(expressions)
    }

    fn parse_property_list(
        &mut self,
        ctx: &mut ParseContext<'a>,
    ) -> Result<Vec<(String, ParseResult)>, ParseError> {
        let mut properties = Vec::new();
        while !ctx.match_token(TokenKind::RBrace) {
            let key = ctx.expect_identifier()?;
            ctx.expect_token(TokenKind::Colon)?;
            let value = self.parse_expression(ctx)?;
            properties.push((key, value));
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        Ok(properties)
    }

    fn parse_case_expression(
        &mut self,
        start_pos: Position,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        ctx.expect_token(TokenKind::Case)?;

        let test_expr = if ctx.peek_token().kind != TokenKind::When {
            Some(self.parse_expression(ctx)?.expr)
        } else {
            None
        };

        let mut conditions = Vec::new();
        while ctx.match_token(TokenKind::When) {
            let when_expr = self.parse_expression(ctx)?;
            ctx.expect_token(TokenKind::Then)?;
            let then_expr = self.parse_expression(ctx)?;
            conditions.push((when_expr.expr, then_expr.expr));
        }

        let default = if ctx.match_token(TokenKind::Else) {
            Some(self.parse_expression(ctx)?.expr)
        } else {
            None
        };

        ctx.expect_token(TokenKind::End)?;

        let span = ctx.merge_span(start_pos, ctx.current_position());
        Ok(ParseResult {
            expr: Expression::case(test_expr, conditions, default),
            span,
        })
    }

    fn parse_list_comprehension(
        &mut self,
        start_pos: Position,
        ctx: &mut ParseContext<'a>,
    ) -> Result<ParseResult, ParseError> {
        let variable = ctx.expect_identifier()?;
        ctx.expect_token(TokenKind::In)?;
        let source = self.parse_expression(ctx)?.expr;

        let (filter, map) = if ctx.match_token(TokenKind::Pipe) {
            let map_expr = self.parse_expression(ctx)?;
            (None, Some(map_expr.expr))
        } else if ctx.match_token(TokenKind::Where) {
            let filter_expr = self.parse_expression(ctx)?;
            let map_expr = if ctx.match_token(TokenKind::Pipe) {
                Some(self.parse_expression(ctx)?.expr)
            } else {
                None
            };
            (Some(filter_expr.expr), map_expr)
        } else {
            (None, None)
        };

        ctx.expect_token(TokenKind::RBracket)?;

        let span = ctx.merge_span(start_pos, ctx.current_position());
        Ok(ParseResult {
            expr: Expression::list_comprehension(variable, source, filter, map),
            span,
        })
    }

    fn parse_subquery_body(&mut self, ctx: &mut ParseContext<'a>) -> Result<SubqueryBody, ParseError> {
        let start_pos = ctx.current_position();
        let mut patterns = Vec::new();
        let mut where_clause = None;
        let mut return_expr = None;

        if ctx.match_token(TokenKind::Match) {
            let pattern_str = self.parse_pattern_string(ctx)?;
            patterns.push(pattern_str);
        }

        if ctx.match_token(TokenKind::Where) {
            let expr = self.parse_expression(ctx)?;
            where_clause = Some(Box::new(expr.expr));
        }

        if ctx.match_token(TokenKind::Return) {
            let expr = self.parse_expression(ctx)?;
            return_expr = Some(Box::new(expr.expr));
        }

        let is_correlated = false;

        Ok(SubqueryBody {
            patterns,
            where_clause,
            return_expr,
            is_correlated,
        })
    }

    fn parse_pattern_string(&mut self, ctx: &mut ParseContext<'a>) -> Result<String, ParseError> {
        let start_pos = ctx.current_position();
        let mut pattern = String::new();

        while !ctx.match_token(TokenKind::RBrace)
            && !ctx.match_token(TokenKind::Where)
            && !ctx.match_token(TokenKind::Return)
            && !ctx.match_token(TokenKind::Match)
        {
            pattern.push_str(&ctx.current_token().lexeme);
            pattern.push(' ');
            ctx.next_token();
        }

        if pattern.is_empty() {
            return Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "Empty pattern in subquery".to_string(),
                start_pos,
            ));
        }

        Ok(pattern.trim().to_string())
    }

    fn match_identifier_token(&mut self, ctx: &mut ParseContext<'a>, expected: &str) -> bool {
        if let TokenKind::Identifier(s) = &ctx.current_token().kind {
            if s.eq_ignore_ascii_case(expected) {
                ctx.next_token();
                return true;
            }
        }
        false
    }

    pub fn set_compat_mode(&mut self, _enabled: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_expression() {
        let input = "1 + 2 * 3";
        let ctx = &mut ParseContext::new(input);
        let mut parser = ExprParser::new(ctx);
        let result = parser.parse_expression(ctx);
        assert!(result.is_ok());
        let parse_result = result.expect("Simple expression parsing should succeed");
        // Verify that the structure of the expression is correct, without checking the specific precedence of the operators.
        assert!(matches!(parse_result.expr, Expression::Binary { .. }));
    }

    #[test]
    fn test_parse_parenthesized_expression() {
        let input = "(1 + 2) * 3";
        let ctx = &mut ParseContext::new(input);
        let mut parser = ExprParser::new(ctx);
        let result = parser.parse_expression(ctx);
        assert!(result.is_ok());
        let parse_result = result.expect("Parsing a bracketed expression should succeed");
        // Verify that the structure of the expression is correct, without checking the specific precedence of the operators.
        assert!(matches!(parse_result.expr, Expression::Binary { .. }));
    }
}
