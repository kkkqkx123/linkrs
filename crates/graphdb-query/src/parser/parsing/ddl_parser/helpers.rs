use graphdb_core::types::PropertyDef;
use graphdb_core::{ArrayTypeInfo, NullType, StructTypeInfo, Value};
use crate::parser::ast::types::DataType;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::expr_parser::parse_expression;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;
use std::sync::Arc;

use super::DdlParser;

impl DdlParser {
    pub fn parse_property_defs(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Vec<PropertyDef>, ParseError> {
        let mut defs = Vec::new();
        if ctx.match_token(TokenKind::LParen) {
            while !ctx.check_token(TokenKind::RParen) {
                let name = ctx.expect_identifier()?;
                let _ = ctx.match_token(TokenKind::Colon);
                let mut serial = false;
                let dtype = if ctx.match_token(TokenKind::Serial) {
                    serial = true;
                    DataType::BigInt
                } else {
                    self.parse_data_type(ctx)?
                };
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
                if serial {
                    // SERIAL columns are implicitly NOT NULL.
                    nullable = false;
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
                    serial,
                });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
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

        use crate::executor::expression::evaluation_context::DefaultExpressionContext;
        use crate::executor::expression::evaluator::ExpressionEvaluator;

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
        let mut serial = false;
        let dtype = if ctx.match_token(TokenKind::Serial) {
            serial = true;
            DataType::BigInt
        } else {
            self.parse_data_type(ctx)?
        };
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
        if serial {
            // SERIAL columns are implicitly NOT NULL.
            nullable = false;
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
            serial,
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
        self.parse_data_type_inner(ctx, 0)
    }

    /// Parse a scalar type from a keyword token. The canonical type name is
    /// resolved through the core `DataType::from_str` parser (single source
    /// of truth for the keyword -> type mapping, including alias rulings).
    fn scalar_type_from_keyword(
        &mut self,
        ctx: &mut ParseContext,
        canonical: &str,
    ) -> Result<DataType, ParseError> {
        ctx.next_token();
        canonical.parse::<DataType>().map_err(|e| {
            ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Unknown data type: {}", e.name),
                ctx.current_position(),
            )
        })
    }

    /// Maximum STRUCT/ARRAY nesting depth (prevents stack overflow on
    /// maliciously nested type declarations).
    const MAX_COMPOSITE_TYPE_DEPTH: usize = 16;

    fn parse_data_type_inner(
        &mut self,
        ctx: &mut ParseContext,
        depth: usize,
    ) -> Result<DataType, ParseError> {
        let token = ctx.current_token();
        match token.kind {
            TokenKind::Struct => {
                ctx.next_token();
                if depth >= Self::MAX_COMPOSITE_TYPE_DEPTH {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "STRUCT nesting exceeds the maximum depth of {}",
                            Self::MAX_COMPOSITE_TYPE_DEPTH
                        ),
                        ctx.current_position(),
                    ));
                }
                if !ctx.match_token(TokenKind::Lt) {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "STRUCT requires '<' after keyword".to_string(),
                        ctx.current_position(),
                    ));
                }
                let mut fields = Vec::new();
                while !ctx.match_token(TokenKind::Gt) {
                    let field_name = ctx.expect_identifier()?;
                    let field_type = self.parse_data_type_inner(ctx, depth + 1)?;
                    fields.push((field_name, field_type));
                    if !ctx.match_token(TokenKind::Comma) {
                        if !ctx.match_token(TokenKind::Gt) {
                            return Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "STRUCT expects ',' or '>' between fields".to_string(),
                                ctx.current_position(),
                            ));
                        }
                        break;
                    }
                }
                Ok(DataType::Struct(Arc::new(StructTypeInfo::new(fields))))
            }
            TokenKind::Array => {
                ctx.next_token();
                if depth >= Self::MAX_COMPOSITE_TYPE_DEPTH {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "ARRAY nesting exceeds the maximum depth of {}",
                            Self::MAX_COMPOSITE_TYPE_DEPTH
                        ),
                        ctx.current_position(),
                    ));
                }
                if !ctx.match_token(TokenKind::Lt) {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "ARRAY requires '<' after keyword".to_string(),
                        ctx.current_position(),
                    ));
                }
                let element = self.parse_data_type_inner(ctx, depth + 1)?;
                if !ctx.match_token(TokenKind::Gt) {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "ARRAY expects '>' after element type".to_string(),
                        ctx.current_position(),
                    ));
                }
                let len = if ctx.match_token(TokenKind::LParen) {
                    if let TokenKind::IntegerLiteral(n) = ctx.current_token().kind {
                        ctx.next_token();
                        if !ctx.match_token(TokenKind::RParen) {
                            return Err(ParseError::new(
                                ParseErrorKind::SyntaxError,
                                "ARRAY length expects ')'".to_string(),
                                ctx.current_position(),
                            ));
                        }
                        Some(n as usize)
                    } else {
                        return Err(ParseError::new(
                            ParseErrorKind::SyntaxError,
                            "ARRAY length requires an integer".to_string(),
                            ctx.current_position(),
                        ));
                    }
                } else {
                    None
                };
                Ok(DataType::Array(Arc::new(ArrayTypeInfo::new(element, len))))
            }
            TokenKind::Int
            | TokenKind::Int8
            | TokenKind::Int16
            | TokenKind::Int32
            | TokenKind::Int64
            | TokenKind::Float
            | TokenKind::Double
            | TokenKind::String
            | TokenKind::Bool
            | TokenKind::Date
            | TokenKind::Time
            | TokenKind::Timestamp
            | TokenKind::Datetime
            | TokenKind::Geography
            | TokenKind::List
            | TokenKind::Map
            | TokenKind::Set
            | TokenKind::UUID
            | TokenKind::Text
            | TokenKind::Null => {
                let canonical = token.lexeme.to_ascii_uppercase();
                self.scalar_type_from_keyword(ctx, &canonical)
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
                    // All other type names (including aliases) are resolved by
                    // the core `DataType::from_str` parser (single source of
                    // truth for the keyword -> type mapping).
                    _ => type_name.parse::<DataType>().map_err(|e| {
                        ParseError::new(
                            ParseErrorKind::SyntaxError,
                            format!("Unknown data type: {}", e.name),
                            ctx.current_position(),
                        )
                    }),
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
