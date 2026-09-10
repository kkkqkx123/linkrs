//! Statement dispatch: keyword-based and token-based routing to the sub-parsers.

use super::admin;
use super::database;
use super::group_by;
use super::misc;
use crate::parser::TokenKind;
use crate::parser::ast::stmt::*;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::parsing::{
    ddl_parser::DdlParser, dml_parser::DmlParser, explain_parser::ExplainParser,
    session_parser::SessionParser, show_parser::ShowParser, transaction_parser::TransactionParser,
    traversal_parser::TraversalParser, user_parser::UserParser, util_stmt_parser::UtilStmtParser,
};

/// Dispatch statements recognized by a leading keyword (MIGRATE, COMMENT,
/// CHECKPOINT, LOAD, INSTALL, CALL, EXPORT, ...). Returns `None` when the
/// current token does not start any keyword-dispatched statement.
pub(super) fn parse_keyword_statement(ctx: &mut ParseContext) -> Option<Result<Stmt, ParseError>> {
    if ctx.check_keyword("MIGRATE") {
        return Some(admin::parse_migrate_statement(ctx));
    }
    if ctx.check_keyword("COMMENT") {
        return Some(admin::parse_comment_on_statement(ctx));
    }
    if ctx.check_keyword("CHECKPOINT") {
        return Some(admin::parse_checkpoint_statement(ctx));
    }
    if ctx.check_keyword("LOAD") {
        if ctx.check_keyword_sequence(&["LOAD", "EXTENSION"]) {
            return Some(admin::parse_extension_statement(ctx));
        }
        return Some(admin::parse_load_from_statement(ctx));
    }
    if ctx.check_keyword("INSTALL")
        || ctx.check_keyword("UNINSTALL")
        || ctx.check_keyword_sequence(&["UPDATE", "EXTENSION"])
    {
        return Some(admin::parse_extension_statement(ctx));
    }
    if ctx.check_keyword("CALL") {
        return Some(database::parse_in_query_call_statement(ctx));
    }
    if ctx.check_keyword("EXPORT") {
        return Some(database::parse_export_database_statement(ctx));
    }
    if ctx.check_keyword("IMPORT") {
        return Some(database::parse_import_database_statement(ctx));
    }
    if ctx.check_keyword("ATTACH") {
        return Some(database::parse_attach_database_statement(ctx));
    }
    if ctx.current_token().kind == TokenKind::Detach
        && !ctx.check_keyword_sequence(&["DETACH", "DELETE"])
    {
        return Some(database::parse_detach_database_statement(ctx));
    }
    None
}

/// Dispatch statements recognized by their leading token.
pub(super) fn parse_token_statement(ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
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
        TokenKind::Delete | TokenKind::Detach => DmlParser::new().parse_delete_statement(ctx),
        TokenKind::Update => misc::parse_update_statement_extended(ctx),
        TokenKind::Upsert => DmlParser::new().parse_upsert_statement(ctx),
        TokenKind::Merge => DmlParser::new().parse_merge_statement(ctx),

        // DDL statements or Cypher CREATE data statements
        TokenKind::Create => misc::parse_create_statement_extended(ctx),
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
        TokenKind::Group => group_by::parse_group_by_statement(ctx),
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
            misc::parse_fulltext_statement(ctx)
        }

        // Variable assignment statement ($var = statement)
        TokenKind::Dollar => misc::parse_assignment_statement(ctx),

        _ => Err(ParseError::new(
            ParseErrorKind::UnexpectedToken,
            format!("Unexpected token: {:?}", token.kind),
            ctx.current_position(),
        )),
    }
}
