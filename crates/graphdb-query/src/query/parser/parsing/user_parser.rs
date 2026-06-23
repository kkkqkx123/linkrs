//! User Management Statement Parsing Module
//!
//! Responsible for parsing statements related to user management, including CREATE USER, ALTER USER, DROP USER, CHANGE PASSWORD, etc.

use crate::query::parser::ast::stmt::*;
use crate::query::parser::ast::types::Span;
use crate::query::parser::core::error::{ParseError, ParseErrorKind};
use crate::query::parser::core::token::TokenKindExt;
use crate::query::parser::parsing::parse_context::ParseContext;
use crate::query::parser::TokenKind;

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

        let username = ctx.expect_identifier()?;

        // Support for the WITH PASSWORD syntax
        ctx.match_token(TokenKind::With);
        ctx.expect_token(TokenKind::Password)?;

        let password = ctx.expect_string_literal()?;

        let mut role = None;
        if ctx.match_token(TokenKind::With) {
            ctx.expect_token(TokenKind::Role)?;
            role = Some(ctx.expect_identifier()?);
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::CreateUser(CreateUserStmt {
            span,
            username,
            password,
            role,
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

        let username = ctx.expect_identifier()?;

        let mut password = None;
        let mut new_role = None;
        let mut is_locked = None;

        // Analyzing the WITH PASSWORD or SET clause
        if ctx.match_token(TokenKind::With) {
            if ctx.match_token(TokenKind::Password) {
                password = Some(ctx.expect_string_literal()?);
            } else if ctx.match_token(TokenKind::Role) {
                new_role = Some(ctx.expect_identifier()?);
            }
        }

        // The SET ROLE = ... and SET LOCKED = ... syntax are also supported.
        while ctx.match_token(TokenKind::Set) {
            if ctx.match_token(TokenKind::Role) {
                ctx.expect_token(TokenKind::Eq)?;
                new_role = Some(ctx.expect_identifier()?);
            } else if ctx.match_token(TokenKind::Locked) {
                ctx.expect_token(TokenKind::Eq)?;
                let value = ctx.expect_identifier()?;
                is_locked = Some(value.to_lowercase() == "true");
            }
        }

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::AlterUser(AlterUserStmt {
            span,
            username,
            password,
            new_role,
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

        let username = ctx.expect_identifier()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

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
        // Parse the optional username (if the next token is an identifier).
        // At this point, the PASSWORD keyword has already been used (i.e., it has been “consumed” in the context of the program or code).
        let username = if ctx.current_token().kind.is_identifier() {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let old_password = ctx.expect_string_literal()?;
        ctx.expect_token(TokenKind::To)?;
        let new_password = ctx.expect_string_literal()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

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
            TokenKind::Admin | TokenKind::AdminRole => {
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
        let space_name = ctx.expect_identifier()?;

        // Analysis of the TO keyword
        ctx.expect_token(TokenKind::To)?;

        // Analyzing the username
        let username = ctx.expect_identifier()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

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
        let space_name = ctx.expect_identifier()?;

        // Analysis of the FROM keyword
        ctx.expect_token(TokenKind::From)?;

        // Analyzing the username
        let username = ctx.expect_identifier()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::Revoke(RevokeStmt {
            span,
            role,
            space_name,
            username,
        }))
    }

    /// Analysis of the DESCRIBE USER statement
    /// Grammar: DESCRIBE USER <username>
    pub fn parse_describe_user_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Desc)?;
        ctx.expect_token(TokenKind::User)?;

        let username = ctx.expect_identifier()?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::DescribeUser(DescribeUserStmt { span, username }))
    }

    /// Analysis of the SHOW USERS statement
    /// Syntax: SHOW USERS
    pub fn parse_show_users_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;
        ctx.expect_token(TokenKind::Users)?;

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowUsers(ShowUsersStmt { span }))
    }

    /// Analysis of the SHOW ROLES statement
    /// Syntax: SHOW ROLES [IN <space_name>]
    pub fn parse_show_roles_statement(
        &mut self,
        ctx: &mut ParseContext,
    ) -> Result<Stmt, ParseError> {
        let start_span = ctx.current_span();
        ctx.expect_token(TokenKind::Show)?;
        ctx.expect_token(TokenKind::Roles)?;

        // Optional IN <space_name> clause
        let space_name = if ctx.match_token(TokenKind::In) {
            Some(ctx.expect_identifier()?)
        } else {
            None
        };

        let end_span = ctx.current_span();
        let span = ctx.merge_span(start_span.start, end_span.end);

        Ok(Stmt::ShowRoles(ShowRolesStmt { span, space_name }))
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
    use super::*;
    use crate::query::parser::Parser;

    #[test]
    fn test_parse_create_user_unicode_username() {
        let query = "CREATE USER 用户 WITH PASSWORD 'password'";
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
        let query = "CREATE USER testuser WITH PASSWORD '密码123'";
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
    fn test_parse_create_user_with_all_roles() {
        for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
            let query = format!("CREATE USER testuser WITH PASSWORD 'pass' WITH ROLE {}", role);
            let mut parser = Parser::new(&query);
            let result = parser.parse();
            assert!(
                result.is_ok(),
                "CREATE USER with role {} should parse: {:?}",
                role,
                result.err()
            );
        }
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
        let query = "ALTER USER 用户 WITH PASSWORD '新密码'";
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
        let query = "DROP USER 用户";
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
        let query = "CHANGE PASSWORD 用户 '旧密码' TO '新密码'";
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
        let query = "GRANT ADMIN ON 空间 TO 用户";
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
        let query = "REVOKE ADMIN ON 空间 FROM 用户";
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
        let query = "DESCRIBE USER 用户";
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
        let query = "SHOW ROLES IN 空间";
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
