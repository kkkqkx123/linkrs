//! Sentence Parsing Module
//!
//! Responsible for parsing various statements, including MATCH, GO, CREATE, DELETE, UPDATE, etc.
//! This module serves as an entry point; it delegates the specific analysis logic to the various sub-modules.

use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::{
    ddl_parser::DdlParser, dml_parser::DmlParser, explain_parser::ExplainParser,
    session_parser::SessionParser, show_parser::ShowParser, transaction_parser::TransactionParser,
    traversal_parser::TraversalParser, user_parser::UserParser, util_stmt_parser::UtilStmtParser,
};
use crate::parser::TokenKind;
use graphdb_core::types::expr::contextual::ContextualExpression;

/// Statement parser - namespace for statement parsing functions.
pub struct StmtParser;

impl StmtParser {
    /// Parse statements (pipeline operators are supported).
    pub fn parse_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let stmt = Self::parse_single_statement(ctx)?;
        Self::parse_pipe_suffix(ctx, stmt)
    }

    /// Analyzing a single statement (without distributing it through any pipelines)
    fn parse_single_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        if ctx.check_keyword("MIGRATE") {
            return Self::parse_migrate_statement(ctx);
        }
        let token = ctx.current_token().clone();
        match token.kind {
            // Graph traversal statement
            TokenKind::Match | TokenKind::Optional => {
                if ctx.check_keyword_sequence(&["MATCH", "VECTOR"]) {
                    return crate::parser::parsing::vector_parser::parse_vector(ctx);
                }
                TraversalParser::new().parse_match_statement(ctx)
            }
            TokenKind::Go => TraversalParser::new().parse_go_statement(ctx),
            TokenKind::Find => TraversalParser::new().parse_find_path_statement(ctx),
            TokenKind::Get => TraversalParser::new().parse_subgraph_statement(ctx),

            // Data modification statements
            TokenKind::Insert => DmlParser::new().parse_insert_statement(ctx),
            TokenKind::Copy => DmlParser::new().parse_copy_statement(ctx),
            TokenKind::Delete => DmlParser::new().parse_delete_statement(ctx),
            TokenKind::Update => Self::parse_update_statement_extended(ctx),
            TokenKind::Upsert => DmlParser::new().parse_upsert_statement(ctx),
            TokenKind::Merge => DmlParser::new().parse_merge_statement(ctx),

            // DDL statements or Cypher CREATE data statements
            TokenKind::Create => Self::parse_create_statement_extended(ctx),
            TokenKind::Drop => DdlParser::new().parse_drop_statement(ctx),
            TokenKind::Desc => DdlParser::new().parse_desc_statement(ctx),
            TokenKind::Alter => DdlParser::new().parse_alter_statement(ctx),

            // User management statements
            TokenKind::CreateUser => UserParser::new().parse_create_user_statement(ctx),
            TokenKind::AlterUser => UserParser::new().parse_alter_user_statement(ctx),
            TokenKind::DropUser => UserParser::new().parse_drop_user_statement(ctx),
            TokenKind::ChangePassword => UserParser::new().parse_change_password_statement(ctx),
            TokenKind::Change => UserParser::new().parse_change_statement(ctx),
            TokenKind::Grant => UserParser::new().parse_grant_statement(ctx),
            TokenKind::Revoke => UserParser::new().parse_revoke_statement(ctx),

            // Tool statements
            TokenKind::Use => UtilStmtParser::new().parse_use_statement(ctx),
            TokenKind::Show => ShowParser::new().parse_show_statement_extended(ctx),
            TokenKind::Explain => ExplainParser::new().parse_explain_statement(ctx),
            TokenKind::Profile => ExplainParser::new().parse_profile_statement(ctx),
            TokenKind::Analyze => ExplainParser::new().parse_analyze_statement(ctx),
            TokenKind::Group => Self::parse_group_by_statement(ctx),
            TokenKind::Kill => SessionParser::new().parse_kill_statement(ctx),
            TokenKind::Fetch => UtilStmtParser::new().parse_fetch_statement(ctx),
            TokenKind::Lookup => {
                if ctx.check_keyword_sequence(&["LOOKUP", "VECTOR"]) {
                    return crate::parser::parsing::vector_parser::parse_vector(ctx);
                }
                UtilStmtParser::new().parse_lookup_statement(ctx)
            }
            TokenKind::Unwind => UtilStmtParser::new().parse_unwind_statement(ctx),
            TokenKind::Return => UtilStmtParser::new().parse_return_statement(ctx),
            TokenKind::With => UtilStmtParser::new().parse_with_statement(ctx),
            TokenKind::Yield => UtilStmtParser::new().parse_yield_statement(ctx),
            TokenKind::Set => UtilStmtParser::new().parse_set_statement(ctx),
            TokenKind::Remove => UtilStmtParser::new().parse_remove_statement(ctx),

            // Transaction statements
            TokenKind::Begin => TransactionParser::new().parse_begin_transaction(ctx),
            TokenKind::Commit => TransactionParser::new().parse_commit_transaction(ctx),
            TokenKind::Rollback => TransactionParser::new().parse_rollback_transaction(ctx),
            TokenKind::Savepoint => TransactionParser::new().parse_savepoint_statement(ctx),
            TokenKind::Release => TransactionParser::new().parse_release_savepoint(ctx),

            // Session variable assignment statement
            TokenKind::Let => SessionParser::new().parse_let_statement(ctx),

            // Full-text search statements
            TokenKind::Search => {
                if ctx.check_keyword_sequence(&["SEARCH", "VECTOR"]) {
                    return crate::parser::parsing::vector_parser::parse_vector(ctx);
                }
                Self::parse_fulltext_statement(ctx)
            }

            // Variable assignment statement ($var = statement)
            TokenKind::Dollar => Self::parse_assignment_statement(ctx),

            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Unexpected token: {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }

    /// Analyzing the pipe suffix (the | operator)
    fn parse_pipe_suffix(ctx: &mut ParseContext, left: Stmt) -> Result<Stmt, ParseError> {
        if ctx.match_token(TokenKind::Pipe) {
            let start_span = left.span();
            let right = Self::parse_pipe_stage(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            Self::parse_pipe_suffix(ctx, pipe_stmt)
        } else if ctx.current_token().kind == TokenKind::With {
            let start_span = left.span();
            let right = UtilStmtParser::new().parse_with_statement(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            Self::parse_pipe_suffix(ctx, pipe_stmt)
        } else if ctx.current_token().kind == TokenKind::Return {
            let start_span = left.span();
            let right = UtilStmtParser::new().parse_return_statement(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            Self::parse_pipe_suffix(ctx, pipe_stmt)
        } else if ctx.current_token().kind == TokenKind::Unwind {
            let start_span = left.span();
            let right = UtilStmtParser::new().parse_unwind_statement(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            Self::parse_pipe_suffix(ctx, pipe_stmt)
        } else if ctx.current_token().kind == TokenKind::Group {
            let start_span = left.span();
            let right = Self::parse_group_by_statement(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            Self::parse_pipe_suffix(ctx, pipe_stmt)
        } else {
            Self::parse_set_operation_suffix(ctx, left)
        }
    }

    /// Parse the right-hand side of a `|` pipe operator.
    fn parse_pipe_stage(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        if ctx.current_token().kind == TokenKind::Where {
            let start_span = ctx.current_span();
            ctx.next_token();
            let expression = Self::parse_expression(ctx)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            return Ok(Stmt::Filter(FilterStmt { span, expression }));
        }
        if ctx.current_token().kind == TokenKind::Group {
            return Self::parse_group_by_statement(ctx);
        }
        if matches!(
            ctx.current_token().kind,
            TokenKind::Identifier(ref word) if word.eq_ignore_ascii_case("COLLECT")
        ) {
            let start_span = ctx.current_span();
            ctx.next_token();
            let mut items = Vec::new();
            loop {
                let expression = Self::parse_expression(ctx)?;
                let alias = if ctx.match_token(TokenKind::As) {
                    Some(ctx.expect_identifier()?)
                } else {
                    None
                };
                items.push(YieldItem { expression, alias });
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            return Ok(Stmt::Collect(CollectStmt { span, items }));
        }
        Self::parse_single_statement(ctx)
    }

    /// Analysis of the GROUP BY statement
    fn parse_group_by_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{GroupByStmt, GroupingType, YieldItem};
        use crate::parser::parsing::clause_parser::ClauseParser;
        use graphdb_core::types::expr::Expression;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Group)?;
        ctx.expect_token(TokenKind::By)?;

        let (group_items, grouping_type) = if ctx.match_token(TokenKind::Rollup) {
            ctx.expect_token(TokenKind::LParen)?;
            let items = Self::parse_grouping_set_items(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            (items.clone(), GroupingType::Rollup(items))
        } else if ctx.match_token(TokenKind::Cube) {
            ctx.expect_token(TokenKind::LParen)?;
            let items = Self::parse_grouping_set_items(ctx)?;
            ctx.expect_token(TokenKind::RParen)?;
            (items.clone(), GroupingType::Cube(items))
        } else if ctx.match_token(TokenKind::Grouping) {
            ctx.expect_token(TokenKind::Sets)?;
            ctx.expect_token(TokenKind::LParen)?;
            let mut sets = Vec::new();
            loop {
                ctx.expect_token(TokenKind::LParen)?;
                let items = Self::parse_grouping_set_items(ctx)?;
                ctx.expect_token(TokenKind::RParen)?;
                sets.push(items);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            ctx.expect_token(TokenKind::RParen)?;
            let all_items: Vec<_> = sets.iter().flatten().cloned().collect();
            (all_items, GroupingType::GroupingSets(sets))
        } else {
            let mut group_items = Vec::new();
            loop {
                let ident = ctx.expect_identifier()?;
                let expr = Expression::Variable(ident);
                let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
                let expr_id = ctx.expression_context().register_expression(expr_meta);
                let contextual_expr = graphdb_core::types::expr::ContextualExpression::new(
                    expr_id,
                    ctx.expression_context_clone(),
                );
                group_items.push(contextual_expr);
                if !ctx.match_token(TokenKind::Comma) {
                    break;
                }
            }
            (group_items, GroupingType::Standard)
        };

        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ClauseParser::new().parse_yield_clause(ctx)?
        } else {
            let items: Vec<YieldItem> = group_items
                .iter()
                .enumerate()
                .map(|(i, expr)| YieldItem {
                    expression: expr.clone(),
                    alias: Some(format!("group_{}", i)),
                })
                .collect();
            crate::parser::ast::stmt::YieldClause {
                span: start_span,
                items,
                where_clause: None,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
            }
        };

        let having_clause = if ctx.match_token(TokenKind::Having) {
            ctx.recover_clause(|_| Ok(None), |c| Self::parse_expression(c).map(Some))?
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::GroupBy(GroupByStmt {
            span,
            group_items,
            grouping_type,
            yield_clause,
            having_clause,
        }))
    }

    /// Parse grouping set items for ROLLUP, CUBE, GROUPING SETS
    fn parse_grouping_set_items(
        ctx: &mut ParseContext,
    ) -> Result<Vec<graphdb_core::types::expr::ContextualExpression>, ParseError> {
        use graphdb_core::types::expr::Expression;
        let mut items = Vec::new();
        loop {
            let ident = ctx.expect_identifier()?;
            let expr = Expression::Variable(ident);
            let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
            let expr_id = ctx.expression_context().register_expression(expr_meta);
            let contextual_expr = graphdb_core::types::expr::ContextualExpression::new(
                expr_id,
                ctx.expression_context_clone(),
            );
            items.push(contextual_expr);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }
        Ok(items)
    }

    /// Analyzing expressions (auxiliary method)
    fn parse_expression(ctx: &mut ParseContext) -> Result<ContextualExpression, ParseError> {
        crate::parser::parsing::expr_parser::parse_expression_with_context(
            ctx,
            ctx.expression_context_clone(),
        )
    }

    /// Analysis of the extended UPDATE statement (including UPDATE CONFIGS)
    fn parse_update_statement_extended(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::UpdateConfigsStmt;
        use crate::parser::parsing::dml_parser::DmlParser;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Update)?;

        if ctx.check_token(TokenKind::Configs) {
            ctx.expect_token(TokenKind::Configs)?;

            let first_ident = ctx.expect_identifier()?;

            let (module, config_name) = if ctx.check_token(TokenKind::Assign) {
                (None, first_ident)
            } else {
                (Some(first_ident), ctx.expect_identifier()?)
            };

            ctx.expect_token(TokenKind::Assign)?;
            let config_value = Self::parse_expression(ctx)?;

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(Stmt::UpdateConfigs(UpdateConfigsStmt {
                span,
                module,
                config_name,
                config_value,
            }))
        } else {
            DmlParser::new().parse_update_after_token(ctx, start_span)
        }
    }

    /// Analysis of the variable assignment statement ($var = statement)
    fn parse_assignment_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::AssignmentStmt;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Dollar)?;

        let var_name = ctx.expect_identifier()?;

        ctx.expect_token(TokenKind::Assign)?;

        let statement = Box::new(Self::parse_statement(ctx)?);

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Assignment(AssignmentStmt {
            span,
            variable: var_name,
            statement,
        }))
    }

    /// Analysis of the extended CREATE statement
    fn parse_create_statement_extended(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::parser::parsing::ddl_parser::DdlParser;
        use crate::parser::parsing::dml_parser::DmlParser;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Create)?;

        if ctx.check_token(TokenKind::LParen) {
            return DmlParser::new().parse_create_data_after_token(ctx, start_span);
        }

        if ctx.check_token(TokenKind::User) {
            return UserParser::new().parse_create_user_statement_after_create(ctx, start_span);
        }

        if ctx.check_keyword("FULLTEXT") {
            return crate::parser::parsing::fulltext_parser::parse_create_fulltext_index_after_create(ctx);
        }

        if ctx.check_keyword("VECTOR") {
            return crate::parser::parsing::vector_parser::parse_create_vector_index_after_create(
                ctx,
            );
        }

        if ctx.check_token(TokenKind::Tag)
            || ctx.check_token(TokenKind::Edge)
            || ctx.check_token(TokenKind::Space)
            || ctx.check_token(TokenKind::Index)
            || ctx.check_token(TokenKind::Sequence)
        {
            return DdlParser::new().parse_create_after_token(ctx, start_span);
        }

        Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "CREATE statement expects '(' (Cypher data creation) or TAG/EDGE/SPACE/INDEX (Schema definition) or USER (user management)".to_string(),
            ctx.current_position(),
        ))
    }

    /// Parse full-text search statements
    fn parse_fulltext_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        crate::parser::parsing::fulltext_parser::parse_fulltext(ctx)
    }

    fn parse_migrate_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.consume_keyword("MIGRATE")?;
        if ctx.check_keyword("PLAN") {
            ctx.consume_keyword("PLAN")?;
            ctx.consume_keyword("FOR")?;
            let is_edge = if ctx.check_keyword("TAG") {
                ctx.consume_keyword("TAG")?;
                false
            } else if ctx.check_keyword("EDGE") {
                ctx.consume_keyword("EDGE")?;
                true
            } else {
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "MIGRATE PLAN expects TAG or EDGE after FOR".to_string(),
                    ctx.current_position(),
                ));
            };
            let label = ctx.expect_identifier()?;
            ctx.consume_keyword("FROM")?;
            ctx.consume_keyword("VERSION")?;
            let from_version = ctx.expect_integer_literal()? as u64;
            ctx.consume_keyword("TO")?;
            let to_version = ctx.expect_integer_literal()? as u64;
            ctx.consume_keyword("IN")?;
            let space = ctx.expect_identifier()?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Migrate(MigrateStmt::Plan(MigratePlanStmt {
                span,
                space,
                label,
                is_edge,
                from_version,
                to_version,
            })))
        } else if ctx.check_keyword("EXECUTE") {
            ctx.consume_keyword("EXECUTE")?;
            let plan_json = ctx.expect_string_literal()?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Migrate(MigrateStmt::Execute(MigrateExecuteStmt {
                span,
                plan_json,
            })))
        } else if ctx.check_keyword("ROLLBACK") {
            ctx.consume_keyword("ROLLBACK")?;
            let plan_json = ctx.expect_string_literal()?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Migrate(MigrateStmt::Rollback(MigrateRollbackStmt {
                span,
                plan_json,
            })))
        } else {
            Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                "MIGRATE expects PLAN, EXECUTE or ROLLBACK".to_string(),
                ctx.current_position(),
            ))
        }
    }

    /// Pipeline after parsing set operation statements, or end of the process.
    fn parse_set_operation_suffix(ctx: &mut ParseContext, left: Stmt) -> Result<Stmt, ParseError> {
        use crate::parser::ast::stmt::{SetOperationStmt, SetOperationType};

        let op_type = if ctx.match_token(TokenKind::Union) {
            if ctx.match_token(TokenKind::All) {
                SetOperationType::UnionAll
            } else {
                SetOperationType::Union
            }
        } else if ctx.match_token(TokenKind::Intersect) {
            SetOperationType::Intersect
        } else if ctx.match_token(TokenKind::SetMinus) {
            SetOperationType::Minus
        } else {
            return Ok(left);
        };

        let start_span = left.span();
        let right = Self::parse_single_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let set_op_stmt = Stmt::SetOperation(SetOperationStmt {
            span,
            op_type,
            left: Box::new(left),
            right: Box::new(right),
        });

        Self::parse_set_operation_suffix(ctx, set_op_stmt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parsing::parse_context::ParseContext;

    fn create_parser_context<'a>(input: &'a str) -> ParseContext<'a> {
        ParseContext::new(input)
    }

    #[test]
    fn test_parse_match_statement() {
        let mut ctx = create_parser_context("MATCH (n:Person) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "MATCH parse failure: {:?}", result.err());
    }

    #[test]
    fn test_parse_go_statement() {
        let mut ctx = create_parser_context("GO 1 STEP FROM \"player100\" OVER follow");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "GO parse failure: {:?}", result.err());
    }

    #[test]
    fn test_parse_create_tag_statement() {
        let mut ctx =
            create_parser_context("CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "CREATE TAG Parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_tag_with_composite_types() {
        let mut ctx = create_parser_context(
            "CREATE TAG Person (id INT, \
             addr STRUCT<city STRING, street STRING, geo STRUCT<lat DOUBLE, lon DOUBLE>>, \
             coords ARRAY<DOUBLE>(3), \
             tags ARRAY<STRING>)",
        );
        let result = StmtParser::parse_statement(&mut ctx);
        let stmt = result.expect("composite type DDL must parse");
        let crate::parser::ast::Stmt::Create(create) = stmt else {
            panic!("expected Create statement");
        };
        let crate::parser::ast::CreateTarget::Tag {
            properties: props, ..
        } = create.target
        else {
            panic!("expected Tag creation");
        };
        let props: Vec<_> = props
            .iter()
            .map(|p| (p.name.clone(), p.data_type.clone()))
            .collect();
        use graphdb_core::{ArrayTypeInfo, DataType, StructTypeInfo};
        use std::sync::Arc;
        assert_eq!(props[0].0, "id");
        assert_eq!(props[0].1, DataType::Int);
        assert_eq!(
            props[1].1,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                ("city".to_string(), DataType::String),
                ("street".to_string(), DataType::String),
                (
                    "geo".to_string(),
                    DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                        ("lat".to_string(), DataType::Double),
                        ("lon".to_string(), DataType::Double),
                    ]))),
                ),
            ])))
        );
        assert_eq!(
            props[2].1,
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3))))
        );
        assert_eq!(
            props[3].1,
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::String, None)))
        );
    }

    #[test]
    fn test_parse_create_tag_composite_nesting_limit() {
        let mut ddl = String::from("CREATE TAG Deep (a ARRAY<");
        for _ in 0..17 {
            ddl.push_str("ARRAY<");
        }
        ddl.push_str("INT");
        for _ in 0..17 {
            ddl.push('>');
        }
        ddl.push_str(">)");
        let mut ctx = create_parser_context(&ddl);
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_err(),
            "over-nested composite type must be rejected"
        );
    }

    #[test]
    fn test_parse_insert_vertex_statement() {
        let mut ctx = create_parser_context(
            "INSERT VERTEX Person(name, age) VALUES \"player100\":(\"Tom\", 18)",
        );
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "INSERT VERTEX parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_delete_vertex_statement() {
        let mut ctx = create_parser_context("DELETE VERTEX \"player100\"");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "DELETE VERTEX parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_use_statement() {
        let mut ctx = create_parser_context("USE test_space");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "USE Parse failure: {:?}", result.err());

        if let Ok(Stmt::Use(stmt)) = result {
            assert_eq!(stmt.space, "test_space");
        } else {
            panic!("Expected Use statement");
        }
    }

    #[test]
    fn test_parse_show_spaces_statement() {
        let mut ctx = create_parser_context("SHOW SPACES");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW SPACES parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_create_space_statement_parses() {
        let mut ctx = create_parser_context("CREATE SPACE IF NOT EXISTS test_space");
        let result = StmtParser::parse_statement(&mut ctx);

        assert!(
            result.is_ok(),
            "CREATE SPACE Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::Create(stmt)) = result {
            match &stmt.target {
                CreateTarget::Space { name, vid_type, .. } => {
                    assert_eq!(name, "test_space");
                    assert_eq!(vid_type, "INT64");
                }
                _ => panic!(
                    "Expect Space to create a goal and actually get {:?}",
                    stmt.target
                ),
            }
            assert!(stmt.if_not_exists);
        } else {
            panic!("The expected Create statement");
        }
    }

    #[test]
    fn test_create_space_with_params_parses() {
        let mut ctx = create_parser_context("CREATE SPACE test_space(vid_type=FIXEDSTRING32)");
        let result = StmtParser::parse_statement(&mut ctx);

        assert!(
            result.is_ok(),
            "CREATE SPACE with params failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Create(stmt)) = result {
            match &stmt.target {
                CreateTarget::Space { name, vid_type, .. } => {
                    assert_eq!(name, "test_space");
                    assert_eq!(vid_type, "FIXEDSTRING32");
                }
                _ => panic!(
                    "Expect Space to create a goal and actually get {:?}",
                    stmt.target
                ),
            }
        } else {
            panic!("The expected Create statement");
        }
    }

    #[test]
    fn test_parse_explain_statement() {
        let mut ctx = create_parser_context("EXPLAIN MATCH (n) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "EXPLAIN Parse failure: {:?}", result.err());

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Table));
        } else {
            panic!("Expected Explain statement");
        }
    }

    #[test]
    fn test_parse_explain_with_format() {
        let mut ctx = create_parser_context("EXPLAIN FORMAT = DOT MATCH (n) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "EXPLAIN FORMAT failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Dot));
            assert!(!stmt.analyze);
        } else {
            panic!("Expected Explain statement");
        }
    }

    #[test]
    fn test_parse_explain_analyze_statement() {
        let mut ctx = create_parser_context("EXPLAIN ANALYZE MATCH (n) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "EXPLAIN ANALYZE failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(stmt.analyze);
        } else {
            panic!("Expected EXPLAIN ANALYZE statement");
        }
    }

    #[test]
    fn test_parse_explain_analyze_with_format() {
        let mut ctx = create_parser_context("EXPLAIN ANALYZE FORMAT = DOT MATCH (n) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "EXPLAIN ANALYZE FORMAT failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(stmt.analyze);
            assert!(matches!(stmt.format, ExplainFormat::Dot));
        } else {
            panic!("Expected EXPLAIN ANALYZE statement");
        }
    }

    #[test]
    fn test_parse_profile_statement() {
        let mut ctx = create_parser_context("PROFILE GO FROM \"player100\" OVER follow");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "PROFILE parse failure: {:?}", result.err());

        if let Ok(Stmt::Profile(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Table));
        } else {
            panic!("Expected Profile statement");
        }
    }

    #[test]
    fn test_parse_analyze_statement() {
        let mut ctx = create_parser_context("ANALYZE");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "ANALYZE parse failure: {:?}", result.err());

        if let Ok(Stmt::Analyze(stmt)) = result {
            assert_eq!(stmt.space, None);
        } else {
            panic!("Expected Analyze statement");
        }
    }

    #[test]
    fn test_parse_analyze_space_statement() {
        let mut ctx = create_parser_context("ANALYZE SPACE basketball");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "ANALYZE SPACE parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::Analyze(stmt)) = result {
            assert_eq!(stmt.space.as_deref(), Some("basketball"));
        } else {
            panic!("Expected Analyze statement");
        }
    }

    #[test]
    fn test_parse_profile_with_format() {
        let mut ctx = create_parser_context("PROFILE FORMAT = TABLE MATCH (n) RETURN n");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "PROFILE FORMAT failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Profile(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Table));
        } else {
            panic!("Expected Profile statement");
        }
    }

    #[test]
    fn test_parse_group_by_statement() {
        let mut ctx = create_parser_context("GROUP BY category YIELD category");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "GROUP BY Parse failure: {:?}", result.err());

        if let Ok(Stmt::GroupBy(stmt)) = result {
            assert_eq!(stmt.group_items.len(), 1);
            assert_eq!(stmt.yield_clause.items.len(), 1);
            assert!(stmt.having_clause.is_none());
        } else {
            panic!("Expected GroupBy statement");
        }
    }

    #[test]
    fn test_parse_group_by_multiple_items() {
        let mut ctx = create_parser_context("GROUP BY category, type YIELD category, type");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "GROUP BY multiple field parsing failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::GroupBy(stmt)) = result {
            assert_eq!(stmt.group_items.len(), 2);
            assert_eq!(stmt.yield_clause.items.len(), 2);
        } else {
            panic!("Expected GroupBy statement");
        }
    }

    #[test]
    fn test_parse_show_sessions() {
        let mut ctx = create_parser_context("SHOW SESSIONS");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW SESSIONS Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowSessions(_)) = result {
        } else {
            panic!("Expected ShowSessions statement");
        }
    }

    #[test]
    fn test_parse_show_queries() {
        let mut ctx = create_parser_context("SHOW QUERIES");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW QUERIES Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowQueries(_)) = result {
        } else {
            panic!("Expected ShowQueries statement");
        }
    }

    #[test]
    fn test_parse_kill_query() {
        let mut ctx = create_parser_context("KILL QUERY 123, 456");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "KILL QUERY Parsing failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::KillQuery(stmt)) = result {
            assert_eq!(stmt.session_id, 123);
            assert_eq!(stmt.plan_id, 456);
        } else {
            panic!("Expected KillQuery statement");
        }
    }

    #[test]
    fn test_parse_begin_transaction_access_modes() {
        let mut ctx = create_parser_context("BEGIN TRANSACTION");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "BEGIN TRANSACTION parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::BeginTransaction(stmt)) = result {
            assert_eq!(stmt.read_only, None);
        } else {
            panic!("Expected a BeginTransaction statement");
        }

        let mut ctx = create_parser_context("BEGIN READ ONLY");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "BEGIN READ ONLY parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::BeginTransaction(stmt)) = result {
            assert_eq!(stmt.read_only, Some(true));
        } else {
            panic!("Expected a BeginTransaction statement");
        }

        let mut ctx = create_parser_context("BEGIN TRANSACTION READ WRITE");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "BEGIN TRANSACTION READ WRITE parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::BeginTransaction(stmt)) = result {
            assert_eq!(stmt.read_only, Some(false));
        } else {
            panic!("Expected a BeginTransaction statement");
        }

        let mut ctx = create_parser_context("BEGIN READ");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_err(),
            "BEGIN READ should be rejected as an incomplete access mode"
        );
    }

    #[test]
    fn test_parse_show_configs() {
        let mut ctx = create_parser_context("SHOW CONFIGS");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW CONFIGS Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowConfigs(stmt)) = result {
            assert!(stmt.module.is_none());
        } else {
            panic!("Expected ShowConfigs statement");
        }
    }

    #[test]
    fn test_parse_show_configs_with_module() {
        let mut ctx = create_parser_context("SHOW CONFIGS storage");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW CONFIGS storage Parse failed: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowConfigs(stmt)) = result {
            assert_eq!(stmt.module, Some("storage".to_string()));
        } else {
            panic!("Expected ShowConfigs statement");
        }
    }

    #[test]
    fn test_parse_update_configs() {
        let mut ctx = create_parser_context("UPDATE CONFIGS max_connections = 100");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "UPDATE CONFIGS parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::UpdateConfigs(stmt)) = result {
            assert!(stmt.module.is_none());
            assert_eq!(stmt.config_name, "max_connections");
        } else {
            panic!("Expected UpdateConfigs statement");
        }
    }

    #[test]
    fn test_parse_update_configs_with_module() {
        let mut ctx = create_parser_context("UPDATE CONFIGS storage cache_size = 1024");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "UPDATE CONFIGS storage Parse failed: {:?}",
            result.err()
        );

        if let Ok(Stmt::UpdateConfigs(stmt)) = result {
            assert_eq!(stmt.module, Some("storage".to_string()));
            assert_eq!(stmt.config_name, "cache_size");
        } else {
            panic!("Expected UpdateConfigs statement");
        }
    }

    #[test]
    fn test_parse_assignment_statement() {
        let mut ctx = create_parser_context("$result = GO FROM \"player100\" OVER follow");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "Variable assignment parsing failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::Assignment(stmt)) = result {
            assert_eq!(stmt.variable, "result");
        } else {
            panic!(
                "Expecting an Assignment statement, you actually get {:?}",
                result
            );
        }
    }

    #[test]
    fn test_parse_let_statement() {
        let mut ctx = create_parser_context("LET $x = 1 + 2");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "LET parse failure: {:?}", result.err());
        if let Ok(Stmt::AssignVariable(stmt)) = result {
            assert_eq!(stmt.name, "x");
            let expr = stmt
                .expression
                .get_expression()
                .expect("expression should resolve");
            assert!(
                matches!(expr, graphdb_core::types::expr::Expression::Binary { .. }),
                "LET RHS should parse as a binary expression, got {:?}",
                expr
            );
        } else {
            panic!("Expected an AssignVariable statement, got {:?}", result);
        }

        let mut ctx = create_parser_context("LET y = 'Alice'");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "LET without $ parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::AssignVariable(stmt)) = result {
            assert_eq!(stmt.name, "y");
        } else {
            panic!("Expected an AssignVariable statement, got {:?}", result);
        }
    }

    #[test]
    fn test_parse_let_statement_errors() {
        let mut ctx = create_parser_context("LET $x");
        let result = StmtParser::parse_statement(&mut ctx);
        let err = result.expect_err("LET without `=` must fail");
        assert!(
            err.to_string().contains("LET requires an assignment"),
            "unexpected error: {}",
            err
        );

        let mut ctx = create_parser_context("LET $ = 1");
        let result = StmtParser::parse_statement(&mut ctx);
        let err = result.expect_err("LET with empty name must fail");
        assert!(
            err.to_string().contains("Invalid session variable name"),
            "unexpected error: {}",
            err
        );

        let mut ctx = create_parser_context("LET $1x = 1");
        let result = StmtParser::parse_statement(&mut ctx);
        let err = result.expect_err("LET with digit-leading name must fail");
        assert!(
            err.to_string().contains("Invalid session variable name"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_parse_rollback_to_savepoint() {
        let mut ctx = create_parser_context("ROLLBACK TO sp1");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "ROLLBACK TO parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::RollbackTransaction(stmt)) = result {
            assert_eq!(stmt.savepoint_name, Some("sp1".to_string()));
        } else {
            panic!("Expected a RollbackTransaction statement, got {:?}", result);
        }

        let mut ctx = create_parser_context("ROLLBACK");
        let result = StmtParser::parse_statement(&mut ctx);
        if let Ok(Stmt::RollbackTransaction(stmt)) = result {
            assert_eq!(stmt.savepoint_name, None);
        } else {
            panic!("Expected a RollbackTransaction statement, got {:?}", result);
        }
    }

    #[test]
    fn test_parse_savepoint_and_release() {
        let mut ctx = create_parser_context("SAVEPOINT sp1");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SAVEPOINT parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::Savepoint(stmt)) = result {
            assert_eq!(stmt.name, "sp1");
        } else {
            panic!("Expected a Savepoint statement, got {:?}", result);
        }

        let mut ctx = create_parser_context("RELEASE SAVEPOINT sp1");
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "RELEASE SAVEPOINT parse failure: {:?}",
            result.err()
        );
        if let Ok(Stmt::ReleaseSavepoint(stmt)) = result {
            assert_eq!(stmt.name, "sp1");
        } else {
            panic!("Expected a ReleaseSavepoint statement, got {:?}", result);
        }
    }

    #[test]
    fn test_parse_union_statement() {
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow UNION GO FROM \"player101\" OVER follow",
        );
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "UNION Parse failure: {:?}", result.err());

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::parser::ast::stmt::SetOperationType::Union
            ));
        } else {
            panic!(
                "Expecting a SetOperation statement, you actually get {:?}",
                result
            );
        }
    }

    #[test]
    fn test_parse_intersect_statement() {
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow INTERSECT GO FROM \"player101\" OVER follow",
        );
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "INTERSECT parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::parser::ast::stmt::SetOperationType::Intersect
            ));
        } else {
            panic!(
                "Expecting a SetOperation statement, you actually get {:?}",
                result
            );
        }
    }

    #[test]
    fn test_parse_minus_statement() {
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow MINUS GO FROM \"player101\" OVER follow",
        );
        let result = StmtParser::parse_statement(&mut ctx);
        assert!(result.is_ok(), "MINUS parse failure: {:?}", result.err());

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::parser::ast::stmt::SetOperationType::Minus
            ));
        } else {
            panic!(
                "Expecting a SetOperation statement, you actually get {:?}",
                result
            );
        }
    }
}
