use crate::core::types::PropertyDef;
use crate::core::{NullType, Value};
use crate::query::parser::ast::types::DataType;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::expr_parser::parse_expression;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

use super::DdlParser;

impl DdlParser {
    pub fn parse_property_defs(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<PropertyDef>, ParseError> {
        let mut defs = Vec::new();
        if ctx.match_token(TokenKind::LParen) {
            while !ctx.match_token(TokenKind::RParen) {
                let name = ctx.expect_identifier()?;
                let _ = ctx.match_token(TokenKind::Colon);
                let dtype = self.parse_data_type(ctx)?;
                let mut nullable = true;
                if ctx.check_token(TokenKind::Not) {
                    ctx.next_token();
                    if ctx.check_token(TokenKind::Null) {
                        ctx.next_token();
                        nullable = false;
                    }
                } else if ctx.match_token(TokenKind::Null) {
                    nullable = true;
                }
                let mut default = None;
                if ctx.match_token(TokenKind::Default) {
                    default = Some(self.parse_value_literal(ctx)?);
                }
                let mut comment = None;
                if ctx.match_token(TokenKind::Comment) {
                    comment = Some(ctx.expect_string_literal()?);
                }
                defs.push(PropertyDef {
                    name,
                    data_type: dtype,
                    nullable,
                    default,
                    comment,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }
        Ok(defs)
    }

    fn parse_value_literal(&mut self, ctx: &mut ParseContext) -> Result<Value, ParseError> {
        let token_kind = ctx.current_token().kind.clone();

        if matches!(token_kind, TokenKind::Identifier(_))
            && ctx.peek_token().kind == TokenKind::LParen
        {
            return self.parse_and_eval_function_call(ctx);
        }

        match token_kind {
            TokenKind::StringLiteral(s) => {
                ctx.next_token();
                Ok(Value::string(s))
            }
            TokenKind::IntegerLiteral(n) => {
                ctx.next_token();
                Ok(Value::BigInt(n))
            }
            TokenKind::FloatLiteral(f) => {
                ctx.next_token();
                Ok(Value::Double(f))
            }
            TokenKind::BooleanLiteral(b) => {
                ctx.next_token();
                Ok(Value::Bool(b))
            }
            TokenKind::Null => {
                ctx.next_token();
                Ok(Value::Null(NullType::Null))
            }
            TokenKind::Minus => {
                ctx.next_token();
                let inner_token_kind = ctx.current_token().kind.clone();
                match inner_token_kind {
                    TokenKind::IntegerLiteral(n) => {
                        ctx.next_token();
                        Ok(Value::BigInt(-n))
                    }
                    TokenKind::FloatLiteral(f) => {
                        ctx.next_token();
                        Ok(Value::Double(-f))
                    }
                    _ => Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Expected number after minus sign, found {:?}",
                            inner_token_kind
                        ),
                        ctx.current_position(),
                    )),
                }
            }
            _ => Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Unsupported default value type: {:?}", token_kind),
                ctx.current_position(),
            )),
        }
    }

    fn parse_and_eval_function_call(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Value, ParseError> {
        let parse_result = parse_expression(ctx)?;

        use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
        use crate::query::executor::expression::evaluator::ExpressionEvaluator;

        let mut eval_ctx = DefaultExpressionContext::new();
        ExpressionEvaluator::evaluate(&parse_result.expr, &mut eval_ctx).map_err(|e| {
            ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Failed to evaluate DEFAULT expression: {}", e),
                ctx.current_position(),
            )
        })
    }

    pub(super) fn parse_single_property_def(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<PropertyDef, ParseError> {
        let name = ctx.expect_identifier()?;
        ctx.match_token(TokenKind::Colon);
        let dtype = self.parse_data_type(ctx)?;
        let mut nullable = true;
        if ctx.check_token(TokenKind::Not) {
            ctx.next_token();
            if ctx.check_token(TokenKind::Null) {
                ctx.next_token();
                nullable = false;
            }
        } else if ctx.match_token(TokenKind::Null) {
            nullable = true;
        }
        let mut default = None;
        if ctx.match_token(TokenKind::Default) {
            default = Some(self.parse_value_literal(ctx)?);
        }
        let mut comment = None;
        if ctx.match_token(TokenKind::Comment) {
            comment = Some(ctx.expect_string_literal()?);
        }
        Ok(PropertyDef {
            name,
            data_type: dtype,
            nullable,
            default,
            comment,
        })
    }

    pub(super) fn parse_vid_type_value(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<String, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            TokenKind::String => {
                ctx.next_token();
                Ok("STRING".to_string())
            }
            TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64 => {
                ctx.next_token();
                Ok("INT64".to_string())
            }
            TokenKind::Float => {
                ctx.next_token();
                Ok("FLOAT".to_string())
            }
            TokenKind::Double => {
                ctx.next_token();
                Ok("DOUBLE".to_string())
            }
            TokenKind::FixedString => {
                ctx.next_token();
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(length) = ctx.current_token().kind {
                        let len = length;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(format!("FIXED_STRING({})", len))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "FIXED_STRING right parenthesis required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "FIXED_STRING requires length parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok("FIXED_STRING(32)".to_string())
                }
            }
            TokenKind::Identifier(ref s) => {
                let type_name = s.clone();
                ctx.next_token();
                Ok(type_name.to_uppercase())
            }
            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected vid_type value, found {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }

    pub fn parse_data_type(&mut self, ctx: &mut ParseContext) -> Result<DataType, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64 => {
                ctx.next_token();
                Ok(DataType::Int)
            }
            TokenKind::Float => {
                ctx.next_token();
                Ok(DataType::Float)
            }
            TokenKind::Double => {
                ctx.next_token();
                Ok(DataType::Double)
            }
            TokenKind::String => {
                ctx.next_token();
                Ok(DataType::String)
            }
            TokenKind::FixedString => {
                ctx.next_token();
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                        let length = len as usize;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(DataType::FixedString(length))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "FIXED_STRING Right bracket required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "FIXED_STRING Need length parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok(DataType::FixedString(32))
                }
            }
            TokenKind::Bool => {
                ctx.next_token();
                Ok(DataType::Bool)
            }
            TokenKind::Date => {
                ctx.next_token();
                Ok(DataType::Date)
            }
            TokenKind::Timestamp => {
                ctx.next_token();
                Ok(DataType::Timestamp)
            }
            TokenKind::Datetime => {
                ctx.next_token();
                Ok(DataType::DateTime)
            }
            TokenKind::Geography => {
                ctx.next_token();
                Ok(DataType::Geography)
            }
            TokenKind::KeywordVector => {
                ctx.next_token();
                if ctx.current_token().kind == TokenKind::LParen {
                    ctx.next_token();
                    if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                        let dimension = len as usize;
                        ctx.next_token();
                        if ctx.current_token().kind == TokenKind::RParen {
                            ctx.next_token();
                            Ok(DataType::VectorDense(dimension))
                        } else {
                            Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "VECTOR Right bracket required".to_string(),
                                ctx.current_position(),
                            ))
                        }
                    } else {
                        Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "VECTOR requires dimension parameter".to_string(),
                            ctx.current_position(),
                        ))
                    }
                } else {
                    Ok(DataType::Vector)
                }
            }
            TokenKind::Identifier(ref s) => {
                let type_name = s.clone();
                ctx.next_token();
                match type_name.to_uppercase().as_str() {
                    "INT" | "INTEGER" | "INT8" | "INT16" | "INT32" | "INT64" => Ok(DataType::Int),
                    "FLOAT" => Ok(DataType::Float),
                    "DOUBLE" => Ok(DataType::Double),
                    "STRING" | "VARCHAR" | "TEXT" => Ok(DataType::String),
                    "FIXED_STRING" | "FIXEDSTRING" => {
                        if ctx.current_token().kind == TokenKind::LParen {
                            ctx.next_token();
                            if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                                let length = len as usize;
                                ctx.next_token();
                                if ctx.current_token().kind == TokenKind::RParen {
                                    ctx.next_token();
                                    Ok(DataType::FixedString(length))
                                } else {
                                    Err(ParseError::new(
                                        ParseErrorKind::SyntaxError,
                                        "FIXED_STRING Right bracket required".to_string(),
                                        ctx.current_position(),
                                    ))
                                }
                            } else {
                                Err(ParseError::new(
                                    ParseErrorKind::SyntaxError,
                                    "FIXED_STRING Need length parameter".to_string(),
                                    ctx.current_position(),
                                ))
                            }
                        } else {
                            Ok(DataType::FixedString(32))
                        }
                    }
                    "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
                    "DATE" => Ok(DataType::Date),
                    "TIMESTAMP" => Ok(DataType::Timestamp),
                    "DATETIME" => Ok(DataType::DateTime),
                    "GEOGRAPHY" => Ok(DataType::Geography),
                    "VECTOR" => {
                        if ctx.current_token().kind == TokenKind::LParen {
                            ctx.next_token();
                            if let TokenKind::IntegerLiteral(len) = ctx.current_token().kind {
                                let dimension = len as usize;
                                ctx.next_token();
                                if ctx.current_token().kind == TokenKind::RParen {
                                    ctx.next_token();
                                    Ok(DataType::VectorDense(dimension))
                                } else {
                                    Err(ParseError::new(
                                        ParseErrorKind::SyntaxError,
                                        "VECTOR Right bracket required".to_string(),
                                        ctx.current_position(),
                                    ))
                                }
                            } else {
                                Err(ParseError::new(
                                    ParseErrorKind::SyntaxError,
                                    "VECTOR requires dimension parameter".to_string(),
                                    ctx.current_position(),
                                ))
                            }
                        } else {
                            Ok(DataType::Vector)
                        }
                    }
                    _ => Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!("Unknown data type: {}", type_name),
                        ctx.current_position(),
                    )),
                }
            }
            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Expected data type, discovered {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }
}
