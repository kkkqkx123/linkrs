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
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::Expression as CoreExpression;

mod copy;
mod create;
mod delete;
mod insert;
mod merge;
mod update;

/// Data Modification Parser
pub struct DmlParser;

impl DmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Extract a literal value from a WHERE clause expression matching `id(<var_name>) == <literal>`.
    ///
    /// Returns the extracted value together with the remaining conjunction
    /// terms (if any), so callers can keep the residual predicate as a
    /// regular filter instead of silently discarding it.
    fn extract_id_with_residual(
        ctx: &mut ParseContext,
        where_clause: Option<&ContextualExpression>,
        var_names: &[&str],
    ) -> (Option<linkrs_core::Value>, Option<ContextualExpression>) {
        let Some(root) = where_clause.and_then(|w| w.get_expression()) else {
            return (None, None);
        };
        let mut conjuncts = Vec::new();
        Self::collect_conjuncts(root, &mut conjuncts);
        let mut extracted = None;
        let mut rest = Vec::new();
        for conjunct in conjuncts {
            if extracted.is_none() {
                if let Some(value) = Self::match_id_equality(&conjunct, var_names) {
                    extracted = Some(value);
                    continue;
                }
            }
            rest.push(conjunct);
        }
        (extracted, Self::combine_conjuncts(ctx, rest))
    }

    /// Split a top-level `AND` chain into its conjuncts.
    fn collect_conjuncts(expr: CoreExpression, out: &mut Vec<CoreExpression>) {
        use linkrs_core::types::operators::BinaryOperator;
        match expr {
            CoreExpression::Binary {
                left,
                op: BinaryOperator::And,
                right,
            } => {
                Self::collect_conjuncts(*left, out);
                Self::collect_conjuncts(*right, out);
            }
            other => out.push(other),
        }
    }

    /// Match a single `id(<var>) == <literal>` conjunct.
    fn match_id_equality(
        conjunct: &CoreExpression,
        var_names: &[&str],
    ) -> Option<linkrs_core::Value> {
        use linkrs_core::types::operators::BinaryOperator;
        match conjunct {
            CoreExpression::Binary {
                left,
                op: BinaryOperator::Equal,
                right,
            } => {
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
            _ => None,
        }
    }

    /// Rebuild the residual conjunction and register it in the parse context.
    fn combine_conjuncts(
        ctx: &mut ParseContext,
        conjuncts: Vec<CoreExpression>,
    ) -> Option<ContextualExpression> {
        use linkrs_core::types::operators::BinaryOperator;
        let mut iter = conjuncts.into_iter();
        let mut acc = iter.next()?;
        for next in iter {
            acc = CoreExpression::Binary {
                left: Box::new(acc),
                op: BinaryOperator::And,
                right: Box::new(next),
            };
        }
        let meta = linkrs_core::types::expr::ExpressionMeta::with_span(acc, ctx.current_span());
        let id = ctx.expression_context_clone().register_expression(meta);
        Some(ContextualExpression::new(
            id,
            ctx.expression_context_clone(),
        ))
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
        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
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
