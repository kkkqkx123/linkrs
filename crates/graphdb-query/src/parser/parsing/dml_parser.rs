//! Data Modification Statement Parsing Module
//!
//! Responsible for parsing statements related to data modification, including INSERT, DELETE, UPDATE, MERGE, etc.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::ParseError;
use crate::parser::core::error::ParseErrorKind;
use crate::parser::core::token::TokenKindExt;
use crate::parser::parsing::clause_parser::ClauseParser;
use crate::parser::parsing::expr_parser::parse_expression_with_context;
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::traversal_parser::TraversalParser;
use crate::parser::TokenKind;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression as CoreExpression;
use graphdb_core::types::EdgeDirection;

mod insert;
mod delete;
mod update;
mod merge;
mod create;
mod copy;

/// Data Modification Parser
pub struct DmlParser;

impl DmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Extract a literal value from a WHERE clause expression matching `id(<var_name>) == <literal>`.
    fn extract_id_from_condition(
        where_clause: Option<&ContextualExpression>,
        var_names: &[&str],
    ) -> Option<graphdb_core::Value> {
        let expr = where_clause?.get_expression()?;
        Self::extract_id_from_expr(&expr, var_names)
    }

    fn extract_id_from_expr(
        expr: &CoreExpression,
        var_names: &[&str],
    ) -> Option<graphdb_core::Value> {
        match expr {
            CoreExpression::Binary { left, op, right } => {
                use graphdb_core::types::operators::BinaryOperator;
                match op {
                    BinaryOperator::Equal => {
                        if let CoreExpression::Function { name, args } = left.as_ref() {
                            if name == "id" && args.len() == 1 {
                                if let CoreExpression::Variable(v) = args[0].as_expr() {
                                    if var_names.iter().any(|n| **n == *v) {
                                        if let CoreExpression::Literal(value) = right.as_ref() {
                                            return Some(value.clone());
                                        }
                                    }
                                }
                            }
                        }
                        None
                    }
                    BinaryOperator::And => Self::extract_id_from_expr(left, var_names)
                        .or_else(|| Self::extract_id_from_expr(right, var_names)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Parse the expression
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        parse_expression_with_context(ctx, ctx.expression_context_clone())
    }

    /// Parse property map: {prop1: value1, prop2: value2}
    fn parse_property_map(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let _start_span = ctx.current_span();
        let mut properties = Vec::new();

        if !ctx.check_token(TokenKind::RBrace) {
            loop {
                let key = ctx.expect_identifier()?;
                ctx.expect_token(TokenKind::Colon)?;
                let value = self.parse_expression(ctx)?;
                let value_expr = value
                    .expression()
                    .ok_or_else(|| {
                        ParseError::new_simple(
                            "Expression not registered in context".to_string(),
                            ctx.current_position(),
                        )
                    })?
                    .inner()
                    .clone();
                properties.push((key, value_expr));

                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
        }

        let expr = CoreExpression::Map(properties);
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.expression_context().register_expression(expr_meta);
        Ok(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
    }
}

impl Default for DmlParser {
    fn default() -> Self {
        Self::new()
    }
}
