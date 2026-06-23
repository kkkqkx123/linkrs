//! Sentence Parsing Module
//!
//! Responsible for parsing various statements, including MATCH, GO, CREATE, DELETE, UPDATE, etc.
//! This module serves as an entry point; it delegates the specific analysis logic to the various sub-modules.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::*;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::parsing::{
    ddl_parser::DdlParser, dml_parser::DmlParser, traversal_parser::TraversalParser,
    user_parser::UserParser, util_stmt_parser::UtilStmtParser,
};
use crate::query::parser::TokenKind;

/// Statement parser
pub struct StmtParser;

impl StmtParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse statements (pipeline operators are supported)
    pub fn parse_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let stmt = self.parse_single_statement(ctx)?;
        self.parse_pipe_suffix(ctx, stmt)
    }

    /// Analyzing a single statement (without distributing it through any pipelines)
    fn parse_single_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let token = ctx.current_token().clone();
        match token.kind {
            // Graph traversal statement
            TokenKind::Match | TokenKind::Optional => {
                TraversalParser::new().parse_match_statement(ctx)
            }
            TokenKind::Go => TraversalParser::new().parse_go_statement(ctx),
            TokenKind::Find => TraversalParser::new().parse_find_path_statement(ctx),
            TokenKind::Get => TraversalParser::new().parse_subgraph_statement(ctx),

            // Data modification statements
            TokenKind::Insert => DmlParser::new().parse_insert_statement(ctx),
            TokenKind::Delete => DmlParser::new().parse_delete_statement(ctx),
            TokenKind::Update => self.parse_update_statement_extended(ctx),
            TokenKind::Upsert => DmlParser::new().parse_upsert_statement(ctx),
            TokenKind::Merge => DmlParser::new().parse_merge_statement(ctx),

            // DDL statements or Cypher CREATE data statements
            TokenKind::Create => self.parse_create_statement_extended(ctx),
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
            TokenKind::Show => self.parse_show_statement_extended(ctx),
            TokenKind::Explain => self.parse_explain_statement(ctx),
            TokenKind::Profile => self.parse_profile_statement(ctx),
            TokenKind::Group => self.parse_group_by_statement(ctx),
            TokenKind::Kill => self.parse_kill_statement(ctx),
            TokenKind::Fetch => UtilStmtParser::new().parse_fetch_statement(ctx),
            TokenKind::Lookup => UtilStmtParser::new().parse_lookup_statement(ctx),
            TokenKind::Unwind => UtilStmtParser::new().parse_unwind_statement(ctx),
            TokenKind::Return => UtilStmtParser::new().parse_return_statement(ctx),
            TokenKind::With => UtilStmtParser::new().parse_with_statement(ctx),
            TokenKind::Yield => UtilStmtParser::new().parse_yield_statement(ctx),
            TokenKind::Set => UtilStmtParser::new().parse_set_statement(ctx),
            TokenKind::Remove => UtilStmtParser::new().parse_remove_statement(ctx),

            // Transaction statements
            TokenKind::Begin => self.parse_begin_transaction(ctx),
            TokenKind::Commit => self.parse_commit_transaction(ctx),
            TokenKind::Rollback => self.parse_rollback_transaction(ctx),

            // Full-text search statements
            // Check if it's SEARCH VECTOR or SEARCH INDEX
            TokenKind::Search => {
                // Peek ahead to check if it's SEARCH VECTOR
                if ctx.check_keyword_sequence(&["SEARCH", "VECTOR"]) {
                    // It's a vector search, call vector parser
                    return crate::query::parser::parsing::vector_parser::parse_vector(ctx);
                }
                // Otherwise, it's a fulltext search
                self.parse_fulltext_statement(ctx)
            }

            // Variable assignment statement ($var = statement)
            TokenKind::Dollar => self.parse_assignment_statement(ctx),

            _ => Err(ParseError::new(
                ParseErrorKind::UnexpectedToken,
                format!("Unexpected token: {:?}", token.kind),
                ctx.current_position(),
            )),
        }
    }

    /// Analyzing the pipe suffix (the | operator)
    /// Also handles sequential clauses like MATCH ... WITH ... RETURN
    fn parse_pipe_suffix(
        &mut self,
        ctx: &mut ParseContext,
        left: Stmt,
    ) -> Result<Stmt, ParseError> {
        if ctx.match_token(TokenKind::Pipe) {
            let start_span = left.span();
            let right = self.parse_single_statement(ctx)?;
            let end_span = right.span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            let pipe_stmt = Stmt::Pipe(PipeStmt {
                span,
                left: Box::new(left),
                right: Box::new(right),
            });

            self.parse_pipe_suffix(ctx, pipe_stmt)
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

            self.parse_pipe_suffix(ctx, pipe_stmt)
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

            self.parse_pipe_suffix(ctx, pipe_stmt)
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

            self.parse_pipe_suffix(ctx, pipe_stmt)
        } else {
            // Check whether it is a set operation.
            self.parse_set_operation_suffix(ctx, left)
        }
    }

    /// Analyzing the EXPLAIN statement (special handling is required, as it contains sub-statements)
    fn parse_explain_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Explain)?;

        // Analysis of the optional FORMAT clause
        let format = if ctx.match_token(TokenKind::Format) {
            ctx.expect_token(TokenKind::Assign)?;
            let format_name = ctx.expect_identifier()?;
            match format_name.to_uppercase().as_str() {
                "DOT" => ExplainFormat::Dot,
                "TABLE" => ExplainFormat::Table,
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Unknown EXPLAIN format: {}, expects DOT or TABLE",
                            format_name
                        ),
                        ctx.current_position(),
                    ));
                }
            }
        } else {
            ExplainFormat::default()
        };

        let statement = Box::new(self.parse_statement(ctx)?);

        Ok(Stmt::Explain(ExplainStmt {
            span: start_span,
            statement,
            format,
        }))
    }

    /// Analyzing the PROFILE statement
    fn parse_profile_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Profile)?;

        // Parsing the optional FORMAT clause
        let format = if ctx.match_token(TokenKind::Format) {
            ctx.expect_token(TokenKind::Assign)?;
            let format_name = ctx.expect_identifier()?;
            match format_name.to_uppercase().as_str() {
                "DOT" => ExplainFormat::Dot,
                "TABLE" => ExplainFormat::Table,
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        format!(
                            "Unknown PROFILE format: {}, expects DOT or TABLE",
                            format_name
                        ),
                        ctx.current_position(),
                    ));
                }
            }
        } else {
            ExplainFormat::default()
        };

        let statement = Box::new(self.parse_statement(ctx)?);

        Ok(Stmt::Profile(ProfileStmt {
            span: start_span,
            statement,
            format,
        }))
    }

    /// Analysis of the GROUP BY statement
    fn parse_group_by_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::core::types::expr::Expression;
        use crate::query::parser::ast::stmt::{GroupByStmt, YieldItem};
        use crate::query::parser::parsing::clause_parser::ClauseParser;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Group)?;
        ctx.expect_token(TokenKind::By)?;

        // Parse the list of group items (only identifiers are to be parsed).
        let mut group_items = Vec::new();
        loop {
            let ident = ctx.expect_identifier()?;
            let expr = Expression::Variable(ident);
            let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
            let expr_id = ctx.expression_context().register_expression(expr_meta);
            let contextual_expr = crate::core::types::expr::ContextualExpression::new(
                expr_id,
                ctx.expression_context_clone(),
            );
            group_items.push(contextual_expr);
            if !ctx.match_token(TokenKind::Comma) {
                break;
            }
        }

        // Analyzing the YIELD clause
        let yield_clause = if ctx.match_token(TokenKind::Yield) {
            ClauseParser::new().parse_yield_clause(ctx)?
        } else {
            // If YIELD is not available, create a default version that returns all group items.
            let items: Vec<YieldItem> = group_items
                .iter()
                .enumerate()
                .map(|(i, expr)| YieldItem {
                    expression: expr.clone(),
                    alias: Some(format!("group_{}", i)),
                })
                .collect();
            crate::query::parser::ast::stmt::YieldClause {
                span: start_span,
                items,
                where_clause: None,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
            }
        };

        // Analyzing the optional HAVING clause
        let having_clause = if ctx.match_token(TokenKind::Having) {
            Some(self.parse_expression(ctx)?)
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::GroupBy(GroupByStmt {
            span,
            group_items,
            yield_clause,
            having_clause,
        }))
    }

    /// Analyzing expressions (auxiliary method)
    fn parse_expression(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<ContextualExpression, ParseError> {
        let mut expr_parser = crate::query::parser::parsing::ExprParser::new(ctx);
        expr_parser.parse_expression_with_context(ctx, ctx.expression_context_clone())
    }

    /// Analysis of extended SHOW statements (including SESSIONS, QUERIES, and CONFIGS)
    fn parse_show_statement_extended(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{ShowConfigsStmt, ShowQueriesStmt, ShowSessionsStmt};

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;

        // Check the next token.
        if ctx.check_token(TokenKind::Sessions) {
            ctx.expect_token(TokenKind::Sessions)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowSessions(ShowSessionsStmt { span }))
        } else if ctx.check_token(TokenKind::Queries) {
            ctx.expect_token(TokenKind::Queries)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowQueries(ShowQueriesStmt { span }))
        } else if ctx.check_token(TokenKind::Configs) {
            ctx.expect_token(TokenKind::Configs)?;
            // Analysis of the available module names
            let module = if ctx.is_identifier_or_in_token() {
                Some(ctx.expect_identifier()?)
            } else {
                None
            };
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowConfigs(ShowConfigsStmt { span, module }))
        } else if ctx.check_token(TokenKind::Spaces) {
            ctx.expect_token(TokenKind::Spaces)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(crate::query::parser::ast::stmt::ShowStmt {
                span,
                target: crate::query::parser::ast::stmt::ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Tags) {
            ctx.expect_token(TokenKind::Tags)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(crate::query::parser::ast::stmt::ShowStmt {
                span,
                target: crate::query::parser::ast::stmt::ShowTarget::Tags,
            }))
        } else if ctx.check_token(TokenKind::Edges) {
            ctx.expect_token(TokenKind::Edges)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::Show(crate::query::parser::ast::stmt::ShowStmt {
                span,
                target: crate::query::parser::ast::stmt::ShowTarget::Edges,
            }))
        } else if ctx.check_token(TokenKind::Hosts) {
            ctx.expect_token(TokenKind::Hosts)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            // HOSTS is temporarily mapped to Spaces, as this is a single-node implementation.
            Ok(Stmt::Show(crate::query::parser::ast::stmt::ShowStmt {
                span,
                target: crate::query::parser::ast::stmt::ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Parts) {
            ctx.expect_token(TokenKind::Parts)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            // The “PARTS” component is temporarily mapped to the “Spaces” component, because this is a single-node implementation.
            Ok(Stmt::Show(crate::query::parser::ast::stmt::ShowStmt {
                span,
                target: crate::query::parser::ast::stmt::ShowTarget::Spaces,
            }))
        } else if ctx.check_token(TokenKind::Users) {
            ctx.expect_token(TokenKind::Users)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowUsers(
                crate::query::parser::ast::stmt::ShowUsersStmt { span },
            ))
        } else if ctx.check_token(TokenKind::Roles) {
            ctx.expect_token(TokenKind::Roles)?;
            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);
            Ok(Stmt::ShowRoles(
                crate::query::parser::ast::stmt::ShowRolesStmt {
                    span,
                    space_name: None,
                },
            ))
        } else if ctx.check_token(TokenKind::Create) {
            // The SHOW CREATE statement: A unified processing method delegated to UtilStmtParser
            // 支持 SHOW CREATE { SPACE | TAG | EDGE | INDEX } <name>
            UtilStmtParser::new().parse_show_create_internal(ctx, start_span)
        } else {
            Err(ParseError::new(
                ParseErrorKind::SyntaxError,
                format!("Unknown SHOW Target: {:?}", ctx.peek_token().kind),
                ctx.current_position(),
            ))
        }
    }

    /// Analyzing the KILL statement
    fn parse_kill_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::KillQueryStmt;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Kill)?;
        ctx.expect_token(TokenKind::Query)?;

        // Analyzing the `session_id`
        let session_id = ctx.expect_integer_literal()?;

        // Analyzing the use of commas
        ctx.expect_token(TokenKind::Comma)?;

        // Analysis of plan_id
        let plan_id = ctx.expect_integer_literal()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::KillQuery(KillQueryStmt {
            span,
            session_id,
            plan_id,
        }))
    }

    /// Analysis of the extended UPDATE statement (including UPDATE CONFIGS)
    fn parse_update_statement_extended(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::UpdateConfigsStmt;
        use crate::query::parser::parsing::dml_parser::DmlParser;

        // Check whether it is an UPDATE CONFIGS command.
        // Consume the UPDATE token first.
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Update)?;

        if ctx.check_token(TokenKind::Configs) {
            // Analysis of the UPDATE CONFIGS command
            ctx.expect_token(TokenKind::Configs)?;

            // Let’s first analyze the first identifier.
            let first_ident = ctx.expect_identifier()?;

            // Check whether the next token is ‘=’. If it is, the first identifier represents the configuration name.
            // Otherwise, the first identifier is the module name, and the configuration name also needs to be parsed.
            let (module, config_name) = if ctx.check_token(TokenKind::Assign) {
                (None, first_ident)
            } else {
                (Some(first_ident), ctx.expect_identifier()?)
            };

            // Analyzing the equal sign and the value
            ctx.expect_token(TokenKind::Assign)?;
            let config_value = self.parse_expression(ctx)?;

            let end_span = ctx.current_span();
            let span = ctx.merge_span(start_span.start, end_span.end);

            Ok(Stmt::UpdateConfigs(UpdateConfigsStmt {
                span,
                module,
                config_name,
                config_value,
            }))
        } else {
            // It’s not about using the `UPDATE CONFIGS` command; instead, we need to revert to the regular `UPDATE` parsing method.
            // Since we have already used the UPDATE token, we need to call other methods of the DML parser.
            // Here, we directly call the `parse_update_statement` function and handle any errors that may occur.
            // In fact, it should be restructured, but let’s deal with it this way for now.
            DmlParser::new().parse_update_after_token(ctx, start_span)
        }
    }

    /// Analysis of the variable assignment statement ($var = statement)
    fn parse_assignment_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::AssignmentStmt;

        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Dollar)?;

        // Analyzing variable names
        let var_name = ctx.expect_identifier()?;

        // Analyzing the equal sign
        ctx.expect_token(TokenKind::Assign)?;

        // Analyze the sentence on the right side.
        let statement = Box::new(self.parse_statement(ctx)?);

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Assignment(AssignmentStmt {
            span,
            variable: var_name,
            statement,
        }))
    }

    /// Analyzing the extended CREATE statement
    /// Distinguish between DDL CREATE statements (for creating tags, edges, spaces, or indexes) and Cypher CREATE statements for creating data.
    fn parse_create_statement_extended(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::parsing::ddl_parser::DdlParser;
        use crate::query::parser::parsing::dml_parser::DmlParser;

        // Pre-read the next token to determine the type of the CREATE command.
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Create)?;

        // Check whether it is a Cypher-style CREATE statement (starting with ‘(‘).
        if ctx.check_token(TokenKind::LParen) {
            // Cypher CREATE data statement: CREATE (n:Label {props})
            // Since the CREATE token has already been consumed, a special method needs to be called.
            return DmlParser::new().parse_create_data_after_token(ctx, start_span);
        }

        // Check whether it is a CREATE USER statement.
        if ctx.check_token(TokenKind::User) {
            return UserParser::new().parse_create_user_statement_after_create(ctx, start_span);
        }

        // Check whether it is a CREATE FULLTEXT INDEX statement.
        if ctx.check_keyword("FULLTEXT") {
            // Parse as full-text index statement (CREATE already consumed)
            return crate::query::parser::parsing::fulltext_parser::parse_create_fulltext_index_after_create(ctx);
        }

        // Check whether it is a CREATE VECTOR INDEX statement.
        if ctx.check_keyword("VECTOR") {
            // Parse as vector index statement (CREATE already consumed)
            return crate::query::parser::parsing::vector_parser::parse_create_vector_index_after_create(ctx);
        }

        // Check the DDL CREATE type.
        if ctx.check_token(TokenKind::Tag)
            || ctx.check_token(TokenKind::Edge)
            || ctx.check_token(TokenKind::Space)
            || ctx.check_token(TokenKind::Index)
        {
            // DDL CREATE: CREATE TAG/EDGE/SPACE/INDEX
            // Since the CREATE token has already been consumed, a special method of the DDL parser is called.
            return DdlParser::new().parse_create_after_token(ctx, start_span);
        }

        // It is not possible to determine the type; an error has occurred.
        Err(ParseError::new(
            ParseErrorKind::SyntaxError,
            "CREATE statement expects '(' (Cypher data creation) or TAG/EDGE/SPACE/INDEX (Schema definition) or USER (user management)".to_string(),
            ctx.current_position(),
        ))
    }

    /// Parse full-text search statements
    fn parse_fulltext_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        crate::query::parser::parsing::fulltext_parser::parse_fulltext(ctx)
    }

    /// Pipeline after parsing set operation statements, or end of the process.
    fn parse_set_operation_suffix(
        &mut self,
        ctx: &mut ParseContext,
        left: Stmt,
    ) -> Result<Stmt, ParseError> {
        use crate::query::parser::ast::stmt::{SetOperationStmt, SetOperationType};

        // Check whether it is a set operator.
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
            // It is not a set operator; it returns the statement on the left side.
            return Ok(left);
        };

        let start_span = left.span();
        let right = self.parse_single_statement(ctx)?;
        let end_span = right.span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        let set_op_stmt = Stmt::SetOperation(SetOperationStmt {
            span,
            op_type,
            left: Box::new(left),
            right: Box::new(right),
        });

        // Continue to check whether there are any more set operations.
        self.parse_set_operation_suffix(ctx, set_op_stmt)
    }

    /// Parse BEGIN TRANSACTION statement
    fn parse_begin_transaction(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Begin)?;

        // Optional: TRANSACTION keyword
        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::BeginTransaction(BeginTransactionStmt { span }))
    }

    /// Parse COMMIT TRANSACTION statement
    fn parse_commit_transaction(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Commit)?;

        // Optional: TRANSACTION keyword
        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::CommitTransaction(CommitTransactionStmt { span }))
    }

    /// Parse ROLLBACK TRANSACTION statement
    fn parse_rollback_transaction(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Rollback)?;

        // Optional: TRANSACTION keyword
        if ctx.check_token(TokenKind::Transaction) {
            ctx.expect_token(TokenKind::Transaction)?;
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::RollbackTransaction(RollbackTransactionStmt { span }))
    }
}

impl Default for StmtParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::parsing::parse_context::ParseContext;

    fn create_parser_context<'a>(input: &'a str) -> ParseContext<'a> {
        ParseContext::new(input)
    }

    #[test]
    fn test_parse_match_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("MATCH (n:Person) RETURN n");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "MATCH parse failure: {:?}", result.err());
    }

    #[test]
    fn test_parse_go_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("GO 1 STEP FROM \"player100\" OVER follow");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "GO parse failure: {:?}", result.err());
    }

    #[test]
    fn test_parse_create_tag_statement() {
        let mut parser = StmtParser::new();
        let mut ctx =
            create_parser_context("CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "CREATE TAG Parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_insert_vertex_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context(
            "INSERT VERTEX Person(name, age) VALUES \"player100\":(\"Tom\", 18)",
        );
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "INSERT VERTEX parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_delete_vertex_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("DELETE VERTEX \"player100\"");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "DELETE VERTEX parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_use_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("USE test_space");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "USE Parse failure: {:?}", result.err());

        if let Ok(Stmt::Use(stmt)) = result {
            assert_eq!(stmt.space, "test_space");
        } else {
            panic!("“Use statement” is a term commonly used in programming languages to refer to a specific instruction or command that tells the computer how to perform a certain action. For example, in Python, the “print” statement is used to display text on the screen.");
        }
    }

    #[test]
    fn test_parse_show_spaces_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("SHOW SPACES");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW SPACES parse failure: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_create_space_statement_parses() {
        let mut parser = StmtParser::new();

        // It has been tested that the CREATE SPACE statement can be parsed successfully.
        let mut ctx = create_parser_context("CREATE SPACE IF NOT EXISTS test_space");
        let result = parser.parse_statement(&mut ctx);

        // Verification and parsing were successful.
        assert!(
            result.is_ok(),
            "CREATE SPACE Parse failure: {:?}",
            result.err()
        );

        // The verification involves the “Create” statement.
        if let Ok(Stmt::Create(stmt)) = result {
            // The verification confirms that Space created the target.
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
        let mut parser = StmtParser::new();

        // The test shows that the CREATE SPACE statement with parameters can be parsed successfully.
        let mut ctx = create_parser_context("CREATE SPACE test_space(vid_type=FIXEDSTRING32)");
        let result = parser.parse_statement(&mut ctx);

        // Verification and parsing were successful.
        assert!(
            result.is_ok(),
            "CREATE SPACE with params 解析失败: {:?}",
            result.err()
        );

        // The verification involves the “Create” statement.
        if let Ok(Stmt::Create(stmt)) = result {
            // The verification confirms that Space has created the target.
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
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("EXPLAIN MATCH (n) RETURN n");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "EXPLAIN Parse failure: {:?}", result.err());

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Table));
        } else {
            panic!("The phrase “Expect to” is used to indicate that someone has a reasonable expectation or belief about a certain situation or outcome. It suggests that the expectation is based on facts, information, or past experience, and that it is likely to come true. For example, if someone says, “I expect to finish the project by Friday,” they are expressing the belief that they will be able to complete the project by that deadline.");
        }
    }

    #[test]
    fn test_parse_explain_with_format() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("EXPLAIN FORMAT = DOT MATCH (n) RETURN n");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "EXPLAIN FORMAT failed to parse: {:?}",
            result.err()
        );

        if let Ok(Stmt::Explain(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Dot));
        } else {
            panic!("“Expect” is a verb that means to have a belief or hope that something will happen in a particular way. For example, if you expect to finish a project by Friday, you believe that you will be able to complete it by that day. The phrase “explain” is a verb that means to give a clear description or explanation of something so that others can understand it. For example, if someone asks you to explain a complex concept, you need to provide enough information so that they can understand it.");
        }
    }

    #[test]
    fn test_parse_profile_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("PROFILE GO FROM \"player100\" OVER follow");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "PROFILE parse failure: {:?}", result.err());

        if let Ok(Stmt::Profile(stmt)) = result {
            assert!(matches!(stmt.format, ExplainFormat::Table));
        } else {
            panic!("Expected Profile statement");
        }
    }

    #[test]
    fn test_parse_profile_with_format() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("PROFILE FORMAT = TABLE MATCH (n) RETURN n");
        let result = parser.parse_statement(&mut ctx);
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
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("GROUP BY category YIELD category");
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "GROUP BY Parse failure: {:?}", result.err());

        if let Ok(Stmt::GroupBy(stmt)) = result {
            assert_eq!(stmt.group_items.len(), 1);
            assert_eq!(stmt.yield_clause.items.len(), 1);
            assert!(stmt.having_clause.is_none());
        } else {
            panic!("The expectation is that the GroupBy statement will be used.");
        }
    }

    #[test]
    fn test_parse_group_by_multiple_items() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("GROUP BY category, type YIELD category, type");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "GROUP BY multiple field parsing failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::GroupBy(stmt)) = result {
            assert_eq!(stmt.group_items.len(), 2);
            assert_eq!(stmt.yield_clause.items.len(), 2);
        } else {
            panic!("The expectation is that the GroupBy statement will be used.");
        }
    }

    #[test]
    fn test_parse_show_sessions() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("SHOW SESSIONS");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW SESSIONS Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowSessions(_)) = result {
            // Success
        } else {
            panic!("The expectation is that the ShowSessions statement will be executed.");
        }
    }

    #[test]
    fn test_parse_show_queries() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("SHOW QUERIES");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW QUERIES Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowQueries(_)) = result {
            // Success
        } else {
            panic!("The expectation is for the ShowQueries statement to be executed.");
        }
    }

    #[test]
    fn test_parse_kill_query() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("KILL QUERY 123, 456");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "KILL QUERY Parsing failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::KillQuery(stmt)) = result {
            assert_eq!(stmt.session_id, 123);
            assert_eq!(stmt.plan_id, 456);
        } else {
            panic!("The expectation is that the KillQuery statement will be executed.");
        }
    }

    #[test]
    fn test_parse_show_configs() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("SHOW CONFIGS");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW CONFIGS Parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowConfigs(stmt)) = result {
            assert!(stmt.module.is_none());
        } else {
            panic!("The expectation for the ShowConfigs statement");
        }
    }

    #[test]
    fn test_parse_show_configs_with_module() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("SHOW CONFIGS storage");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "SHOW CONFIGS storage Parse failed: {:?}",
            result.err()
        );

        if let Ok(Stmt::ShowConfigs(stmt)) = result {
            assert_eq!(stmt.module, Some("storage".to_string()));
        } else {
            panic!("The expectation for the ShowConfigs statement");
        }
    }

    #[test]
    fn test_parse_update_configs() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("UPDATE CONFIGS max_connections = 100");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "UPDATE CONFIGS parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::UpdateConfigs(stmt)) = result {
            assert!(stmt.module.is_none());
            assert_eq!(stmt.config_name, "max_connections");
        } else {
            panic!("The expectation is for the UpdateConfigs statement to be executed.");
        }
    }

    #[test]
    fn test_parse_update_configs_with_module() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("UPDATE CONFIGS storage cache_size = 1024");
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "UPDATE CONFIGS storage Parse failed: {:?}",
            result.err()
        );

        if let Ok(Stmt::UpdateConfigs(stmt)) = result {
            assert_eq!(stmt.module, Some("storage".to_string()));
            assert_eq!(stmt.config_name, "cache_size");
        } else {
            panic!("The expectation is for the UpdateConfigs statement to be executed.");
        }
    }

    #[test]
    fn test_parse_assignment_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context("$result = GO FROM \"player100\" OVER follow");
        let result = parser.parse_statement(&mut ctx);
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
    fn test_parse_union_statement() {
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow UNION GO FROM \"player101\" OVER follow",
        );
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "UNION Parse failure: {:?}", result.err());

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::query::parser::ast::stmt::SetOperationType::Union
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
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow INTERSECT GO FROM \"player101\" OVER follow",
        );
        let result = parser.parse_statement(&mut ctx);
        assert!(
            result.is_ok(),
            "INTERSECT parse failure: {:?}",
            result.err()
        );

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::query::parser::ast::stmt::SetOperationType::Intersect
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
        let mut parser = StmtParser::new();
        let mut ctx = create_parser_context(
            "GO FROM \"player100\" OVER follow MINUS GO FROM \"player101\" OVER follow",
        );
        let result = parser.parse_statement(&mut ctx);
        assert!(result.is_ok(), "MINUS parse failure: {:?}", result.err());

        if let Ok(Stmt::SetOperation(stmt)) = result {
            assert!(matches!(
                stmt.op_type,
                crate::query::parser::ast::stmt::SetOperationType::Minus
            ));
        } else {
            panic!(
                "Expecting a SetOperation statement, you actually get {:?}",
                result
            );
        }
    }
}
