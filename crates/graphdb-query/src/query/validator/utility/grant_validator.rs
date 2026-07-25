use crate::query::parser::ast::stmt::{Ast, GrantStmt, RevokeStmt, RoleType};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::utility::acl_validator::ValidatedGrant;
use crate::query::validator::{ColumnDef, ValueType};
use crate::query::validator::validator_trait::{ExpressionProps, StatementType, StatementValidator, ValidationResult};
use crate::query::QueryContext;
use std::sync::Arc;

#[derive(Debug)]
pub struct GrantValidator {
    role: RoleType,
    space_name: String,
    username: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl GrantValidator {
    pub fn new() -> Self {
        Self {
            role: RoleType::Guest,
            space_name: String::new(),
            username: String::new(),
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &GrantStmt) -> Result<(), ValidationError> {
        self.role = stmt.role;
        self.space_name = stmt.space_name.clone();
        self.username = stmt.username.clone();

        if self.space_name.is_empty() {
            return Err(ValidationError::new(
                "Space name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedGrant {
        ValidatedGrant {
            role: self.role,
            space_name: self.space_name.clone(),
            username: self.username.clone(),
        }
    }
}

impl StatementValidator for GrantValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let grant_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Grant(grant_stmt) => grant_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected GRANT statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(grant_stmt)?;

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        true
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for GrantValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct RevokeValidator {
    role: RoleType,
    space_name: String,
    username: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl RevokeValidator {
    pub fn new() -> Self {
        Self {
            role: RoleType::Guest,
            space_name: String::new(),
            username: String::new(),
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &RevokeStmt) -> Result<(), ValidationError> {
        self.role = stmt.role;
        self.space_name = stmt.space_name.clone();
        self.username = stmt.username.clone();

        if self.space_name.is_empty() {
            return Err(ValidationError::new(
                "Space name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedGrant {
        ValidatedGrant {
            role: self.role,
            space_name: self.space_name.clone(),
            username: self.username.clone(),
        }
    }
}

impl StatementValidator for RevokeValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let revoke_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Revoke(revoke_stmt) => revoke_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected REVOKE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(revoke_stmt)?;

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        true
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for RevokeValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_validator_new() {
        let validator = GrantValidator::new();
        assert_eq!(validator.username, "");
        assert_eq!(validator.space_name, "");
    }

    #[test]
    fn test_grant_validator_valid() {
        let mut validator = GrantValidator::new();
        let stmt = GrantStmt {
            span: Default::default(),
            role: RoleType::Admin,
            space_name: "test_space".to_string(),
            username: "testuser".to_string(),
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_grant_validator_all_role_types() {
        for role_str in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
            let mut validator = GrantValidator::new();
            let stmt = GrantStmt {
                span: Default::default(),
                role: role_str.parse().unwrap(),
                space_name: "space".to_string(),
                username: "user".to_string(),
            };
            assert!(validator.validate_impl(&stmt).is_ok());
        }
    }

    #[test]
    fn test_revoke_validator_new() {
        let validator = RevokeValidator::new();
        assert_eq!(validator.username, "");
        assert_eq!(validator.space_name, "");
    }

    #[test]
    fn test_revoke_validator_valid() {
        let mut validator = RevokeValidator::new();
        let stmt = RevokeStmt {
            span: Default::default(),
            role: RoleType::Admin,
            space_name: "test_space".to_string(),
            username: "testuser".to_string(),
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }
}
