//! User Management Statement Parsing Module
//!
//! Responsible for parsing statements related to user management, including CREATE USER, ALTER USER, DROP USER, CHANGE PASSWORD, etc.

use crate::parser::ast::stmt::*;
use crate::parser::ast::types::Span;
use crate::parser::core::error::{ParseError, ParseErrorKind};
use crate::parser::parsing::parse_context::ParseContext;
use crate::parser::TokenKind;

/// User Management Parser
pub struct UserParser;

impl UserParser {
    pub fn new() -> Self {
        Self
    }

    /// Analysis of the CREATE USER statement
    pub fn parse_create_user_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::CreateUser)?;
        self.parse_create_user_internal(ctx, start_span)
    }

    /// Analysis of the CREATE USER statement (the CREATE token has already been consumed)
    pub fn parse_create_user_statement_after_create(
        &mut self,
        ctx: &mut ParseContext,
        start_span: Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::User)?;
        self.parse_create_user_internal(ctx, start_span)
    }

    /// Analyzing the internal implementation of the CREATE USER statement
    fn parse_create_user_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: Span,
    ) -> Result<Stmt, ParseError> {
        let mut if_not_exists = false;
        if ctx.match_token(TokenKind::If) {
            ctx.expect_token(TokenKind::Not)?;
            ctx.expect_token(TokenKind::Exists)?;
            if_not_exists = true;
        }

        let username = ctx.expect_dcl_name()?;

        // Support for the WITH PASSWORD syntax
        ctx.match_token(TokenKind::With);
        ctx.expect_token(TokenKind::Password)?;

        let password = ctx.expect_string_literal()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::CreateUser(CreateUserStmt {
            span,
            username,
            password,
            if_not_exists,
        }))
    }

    /// Analysis of the ALTER USER statement
    pub fn parse_alter_user_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::AlterUser)?;
        self.parse_alter_user_internal(ctx, start_span)
    }

    /// Analysis of the internal implementation of the ALTER USER command
    pub fn parse_alter_user_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: Span,
    ) -> Result<Stmt, ParseError> {
        ctx.expect_token(TokenKind::User)?;

        let username = ctx.expect_dcl_name()?;

        let mut password = None;
        let mut is_locked = None;

        // Analyzing the WITH PASSWORD clause
        if ctx.match_token(TokenKind::With) {
            ctx.expect_token(TokenKind::Password)?;
            password = Some(ctx.expect_string_literal()?);
        }

        // The SET LOCKED = <true|false> syntax is also supported.
        while ctx.match_token(TokenKind::Set) {
            if ctx.match_token(TokenKind::Locked) {
                ctx.expect_token(TokenKind::Assign)?;
                is_locked = Some(ctx.expect_boolean_literal()?);
            } else {
                return Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "Expected LOCKED after SET in ALTER USER".to_string(),
                    ctx.current_position(),
                ));
            }
        }

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::AlterUser(AlterUserStmt {
            span,
            username,
            password,
            is_locked,
        }))
    }

    /// Analysis of the DROP USER statement
    pub fn parse_drop_user_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::DropUser)?;

        let mut if_exists = false;
        if ctx.match_token(TokenKind::If) {
            ctx.expect_token(TokenKind::Exists)?;
            if_exists = true;
        }

        let username = ctx.expect_dcl_name()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::DropUser(DropUserStmt {
            span,
            username,
            if_exists,
        }))
    }

    /// Analysis of the CHANGE PASSWORD statement
    pub fn parse_change_password_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::ChangePassword)?;

        self.parse_change_password_internal(ctx, start_span)
    }

    /// Analysis of the internal implementation of the “CHANGE PASSWORD” command
    pub fn parse_change_password_internal(
        &mut self,
        ctx: &mut ParseContext,
        start_span: Span,
    ) -> Result<Stmt, ParseError> {
        // Parse the optional username (if the next token is not the old password literal).
        let username = if matches!(ctx.current_token().kind, TokenKind::StringLiteral(_)) {
            None
        } else {
            Some(ctx.expect_dcl_name()?)
        };

        let old_password = ctx.expect_string_literal()?;
        ctx.expect_token(TokenKind::To)?;
        let new_password = ctx.expect_string_literal()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::ChangePassword(ChangePasswordStmt {
            span,
            username,
            old_password,
            new_password,
        }))
    }

    /// Analysis of the CHANGE statement (currently only CHANGE PASSWORD is supported)
    pub fn parse_change_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Change)?;

        // Check whether it is “CHANGE PASSWORD”.
        if ctx.match_token(TokenKind::Password) {
            return self.parse_change_password_internal(ctx, start_span);
        }

        Err(ParseError::new(
            ParseErrorKind::UnexpectedToken,
            "Expected PASSWORD after CHANGE".to_string(),
            ctx.current_position(),
        ))
    }

    /// Analyzing character types (supporting keyword-based searches)
    fn parse_role_type(&mut self, ctx: &mut ParseContext) -> Result<RoleType, ParseError> {
        let token = ctx.current_token();
        let role_str = match token.kind {
            TokenKind::God => {
                ctx.next_token();
                "GOD".to_string()
            }
            TokenKind::Admin => {
                ctx.next_token();
                "ADMIN".to_string()
            }
            TokenKind::Dba => {
                ctx.next_token();
                "DBA".to_string()
            }
            TokenKind::Guest => {
                ctx.next_token();
                "GUEST".to_string()
            }
            TokenKind::User => {
                ctx.next_token();
                "USER".to_string()
            }
            _ => ctx.expect_identifier()?,
        };

        role_str
            .parse::<RoleType>()
            .map_err(|e| ParseError::new(ParseErrorKind::SyntaxError, e, ctx.current_position()))
    }

    /// Analysis of the GRANT statement
    /// Syntax:  `GRANT ROLE <role_type> ON <space_name> TO <username>`
    pub fn parse_grant_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Grant)?;

        // Analyzing the ROLE keyword (optional)
        let _ = ctx.match_token(TokenKind::Role);

        // Analyzing character types
        let role = self.parse_role_type(ctx)?;

        // Analysis of the ON keyword
        ctx.expect_token(TokenKind::On)?;

        // Analysis of the Space name
        let space_name = ctx.expect_dcl_name()?;

        // Analysis of the TO keyword
        ctx.expect_token(TokenKind::To)?;

        // Analyzing the username
        let username = ctx.expect_dcl_name()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::Grant(GrantStmt {
            span,
            role,
            space_name,
            username,
        }))
    }

    /// Analysis of the REVOKE statement
    /// Syntax: `REVOKE ROLE <role_type> ON <space_name> FROM <username>`
    pub fn parse_revoke_statement(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Revoke)?;

        // Analysis of the ROLE keyword (optional)
        let _ = ctx.match_token(TokenKind::Role);

        // Analyzing character types
        let role = self.parse_role_type(ctx)?;

        // Analysis of the ON keyword
        ctx.expect_token(TokenKind::On)?;

        // Analyzing the name “Space”
        let space_name = ctx.expect_dcl_name()?;

        // Analysis of the FROM keyword
        ctx.expect_token(TokenKind::From)?;

        // Analyzing the username
        let username = ctx.expect_dcl_name()?;

        let span = ctx.span_since(start_span.start);

        Ok(Stmt::Revoke(RevokeStmt {
            span,
            role,
            space_name,
            username,
        }))
    }
}

impl Default for UserParser {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Unit Tests ====================

#[cfg(test)]
mod tests {
    use crate::parser::Parser;

    #[test]
    fn test_parse_create_user_unicode_username() {
        let query = "CREATE USER ユーザー WITH PASSWORD 'password'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Unicode username should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_unicode_password() {
        let query = "CREATE USER testuser WITH PASSWORD 'パスワード123'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Unicode password should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_special_chars_username() {
        let query = "CREATE USER user_123_test WITH PASSWORD 'password'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Username with numbers and underscores should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_special_chars_password() {
        let query = "CREATE USER user WITH PASSWORD 'P@$$w0rd!#%&*'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Password with special chars should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_rejects_with_role() {
        let query = "CREATE USER testuser WITH PASSWORD 'pass' WITH ROLE ADMIN";
        let mut parser = Parser::new(query);
        assert!(
            parser.parse().is_err(),
            "WITH ROLE is no longer part of CREATE USER syntax"
        );
    }

    #[test]
    fn test_parse_alter_user_set_locked() {
        for (value, expected) in [("true", true), ("false", false)] {
            let query = format!("ALTER USER testuser SET LOCKED = {}", value);
            let mut parser = Parser::new(&query);
            let result = parser.parse();
            assert!(
                result.is_ok(),
                "SET LOCKED = {} should parse: {:?}",
                value,
                result.err()
            );
            match result.expect("parsed").ast.stmt() {
                crate::parser::ast::Stmt::AlterUser(stmt) => assert_eq!(stmt.is_locked, Some(expected)),
                other => panic!("expected AlterUser, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_parse_alter_user_set_locked_rejects_non_boolean() {
        for query in [
            "ALTER USER testuser SET LOCKED = yes",
            "ALTER USER testuser SET LOCKED = 1",
        ] {
            let mut parser = Parser::new(query);
            assert!(
                parser.parse().is_err(),
                "{} should be rejected, not silently treated as false",
                query
            );
        }
    }

    #[test]
    fn test_parse_alter_user_set_role_rejected() {
        let mut parser = Parser::new("ALTER USER testuser SET ROLE = DBA");
        assert!(
            parser.parse().is_err(),
            "SET ROLE is no longer part of ALTER USER syntax"
        );
    }

    #[test]
    fn test_parse_create_user_case_insensitive_keywords() {
        let query = "create user testuser with password 'pass'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Lowercase keywords should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_alter_user_unicode() {
        let query = "ALTER USER ユーザー WITH PASSWORD 'ニューパスワード'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "ALTER USER with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_drop_user_unicode() {
        let query = "DROP USER ユーザー";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "DROP USER with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_change_password_unicode() {
        let query = "CHANGE PASSWORD ユーザー 'オールドパスワード' TO 'ニューパスワード'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "CHANGE PASSWORD with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_grant_unicode() {
        let query = "GRANT ADMIN ON スペース TO ユーザー";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "GRANT with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_revoke_unicode() {
        let query = "REVOKE ADMIN ON スペース FROM ユーザー";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "REVOKE with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_describe_user_unicode() {
        let query = "DESCRIBE USER ユーザー";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "DESCRIBE USER with unicode should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_show_roles_with_unicode_space() {
        let query = "SHOW ROLES IN スペース";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "SHOW ROLES IN with unicode space should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_long_username() {
        let long_username = "a".repeat(255);
        let query = format!("CREATE USER {} WITH PASSWORD 'pass'", long_username);
        let mut parser = Parser::new(&query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Long username should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_long_password() {
        let long_password = "a".repeat(1000);
        let query = format!("CREATE USER user WITH PASSWORD '{}'", long_password);
        let mut parser = Parser::new(&query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Long password should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_create_user_empty_like_password() {
        let query = "CREATE USER user WITH PASSWORD 'a'";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Single char password should parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_grant_case_insensitive_role() {
        for role_case in ["ADMIN", "Admin", "admin"] {
            let query = format!("GRANT {} ON space TO user", role_case);
            let mut parser = Parser::new(&query);
            let result = parser.parse();
            assert!(
                result.is_ok(),
                "GRANT with role case {} should parse: {:?}",
                role_case,
                result.err()
            );
        }
    }

    #[test]
    fn test_parse_show_roles_with_and_without_space() {
        let query1 = "SHOW ROLES";
        let query2 = "SHOW ROLES IN myspace";

        let mut parser1 = Parser::new(query1);
        let result1 = parser1.parse();
        assert!(
            result1.is_ok(),
            "SHOW ROLES without space should parse: {:?}",
            result1.err()
        );

        let mut parser2 = Parser::new(query2);
        let result2 = parser2.parse();
        assert!(
            result2.is_ok(),
            "SHOW ROLES with space should parse: {:?}",
            result2.err()
        );
    }

    #[test]
    fn test_parse_whitespace_handling() {
        let queries = vec![
            "CREATE  USER  user  WITH  PASSWORD  'pass'",
            "CREATE\tUSER\tuser\tWITH\tPASSWORD\t'pass'",
            "CREATE\nUSER\nuser\nWITH\nPASSWORD\n'pass'",
        ];

        for query in queries {
            let mut parser = Parser::new(query);
            let result = parser.parse();
            assert!(
                result.is_ok(),
                "Query with extra whitespace should parse: {:?}",
                result.err()
            );
        }
    }
}
