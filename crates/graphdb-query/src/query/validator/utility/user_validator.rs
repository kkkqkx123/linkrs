use crate::query::parser::ast::stmt::{
    AlterUserStmt, Ast, ChangePasswordStmt, CreateUserStmt, DropUserStmt,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::utility::acl_validator::{validate_role, ValidatedUser};
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

#[derive(Debug)]
pub struct CreateUserValidator {
    username: String,
    password: String,
    role: Option<String>,
    if_not_exists: bool,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl CreateUserValidator {
    pub fn new() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            role: None,
            if_not_exists: false,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &CreateUserStmt) -> Result<(), ValidationError> {
        self.username = stmt.username.clone();
        self.password = stmt.password.clone();
        self.role = stmt.role.clone();
        self.if_not_exists = stmt.if_not_exists;

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.password.is_empty() {
            return Err(ValidationError::new(
                "Password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if let Some(ref role) = self.role {
            validate_role(role)?;
        }

        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedUser {
        ValidatedUser {
            username: self.username.clone(),
            role: self.role.clone(),
        }
    }
}

impl StatementValidator for CreateUserValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let create_user_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::CreateUser(create_user_stmt) => create_user_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE USER statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(create_user_stmt)?;

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

impl Default for CreateUserValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct DropUserValidator {
    username: String,
    if_exists: bool,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl DropUserValidator {
    pub fn new() -> Self {
        Self {
            username: String::new(),
            if_exists: false,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &DropUserStmt) -> Result<(), ValidationError> {
        self.username = stmt.username.clone();
        self.if_exists = stmt.if_exists;

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedUser {
        ValidatedUser {
            username: self.username.clone(),
            role: None,
        }
    }
}

impl StatementValidator for DropUserValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let drop_user_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::DropUser(drop_user_stmt) => drop_user_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected DROP USER statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(drop_user_stmt)?;

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

impl Default for DropUserValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct AlterUserValidator {
    username: String,
    password: Option<String>,
    new_role: Option<String>,
    is_locked: Option<bool>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl AlterUserValidator {
    pub fn new() -> Self {
        Self {
            username: String::new(),
            password: None,
            new_role: None,
            is_locked: None,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &AlterUserStmt) -> Result<(), ValidationError> {
        self.username = stmt.username.clone();
        self.password = stmt.password.clone();
        self.new_role = stmt.new_role.clone();
        self.is_locked = stmt.is_locked;

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.password.is_none() && self.new_role.is_none() && self.is_locked.is_none() {
            return Err(ValidationError::new(
                "At least one modification is required".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if let Some(ref role) = self.new_role {
            validate_role(role)?;
        }

        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedUser {
        ValidatedUser {
            username: self.username.clone(),
            role: self.new_role.clone(),
        }
    }
}

impl StatementValidator for AlterUserValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let alter_user_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::AlterUser(alter_user_stmt) => alter_user_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected ALTER USER statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(alter_user_stmt)?;

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

impl Default for AlterUserValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct ChangePasswordValidator {
    username: Option<String>,
    old_password: String,
    new_password: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ChangePasswordValidator {
    pub fn new() -> Self {
        Self {
            username: None,
            old_password: String::new(),
            new_password: String::new(),
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &ChangePasswordStmt) -> Result<(), ValidationError> {
        self.username = stmt.username.clone();
        self.old_password = stmt.old_password.clone();
        self.new_password = stmt.new_password.clone();

        if self.old_password.is_empty() {
            return Err(ValidationError::new(
                "Old password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.new_password.is_empty() {
            return Err(ValidationError::new(
                "New password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.old_password == self.new_password {
            return Err(ValidationError::new(
                "New password must be different from old password".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }
}

impl StatementValidator for ChangePasswordValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let change_password_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ChangePassword(change_password_stmt) => {
                change_password_stmt
            }
            _ => {
                return Err(ValidationError::new(
                    "Expected CHANGE PASSWORD statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(change_password_stmt)?;

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

impl Default for ChangePasswordValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_validator_new() {
        let validator = CreateUserValidator::new();
        assert_eq!(validator.username, "");
        assert_eq!(validator.password, "");
        assert_eq!(validator.role, None);
        assert!(!validator.if_not_exists);
    }

    #[test]
    fn test_validate_role_valid_roles() {
        assert!(validate_role("GOD").is_ok());
        assert!(validate_role("ADMIN").is_ok());
        assert!(validate_role("DBA").is_ok());
        assert!(validate_role("USER").is_ok());
        assert!(validate_role("GUEST").is_ok());
    }

    #[test]
    fn test_validate_role_case_insensitive() {
        assert!(validate_role("god").is_ok());
        assert!(validate_role("Admin").is_ok());
        assert!(validate_role("dba").is_ok());
    }

    #[test]
    fn test_validate_role_invalid() {
        assert!(validate_role("SUPERUSER").is_err());
        assert!(validate_role("ROOT").is_err());
        assert!(validate_role("INVALID").is_err());
    }

    #[test]
    fn test_create_user_validator_empty_username() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "".to_string(),
            password: "pass123".to_string(),
            role: None,
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_create_user_validator_empty_password() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: "".to_string(),
            role: None,
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_create_user_validator_valid_basic() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: "pass123".to_string(),
            role: None,
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_create_user_validator_with_valid_role() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: "pass123".to_string(),
            role: Some("ADMIN".to_string()),
            if_not_exists: true,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_create_user_validator_with_invalid_role() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: "pass123".to_string(),
            role: Some("INVALID_ROLE".to_string()),
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_create_user_validator_unicode_username() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "用户名".to_string(),
            password: "pass123".to_string(),
            role: None,
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_create_user_validator_special_chars_password() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: "P@$$w0rd!2024#特殊".to_string(),
            role: None,
            if_not_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_validated_user_result() {
        let mut validator = CreateUserValidator::new();
        let stmt = CreateUserStmt {
            span: Default::default(),
            username: "alice".to_string(),
            password: "pass".to_string(),
            role: Some("DBA".to_string()),
            if_not_exists: false,
        };
        validator.validate_impl(&stmt).unwrap();
        let result = validator.validated_result();
        assert_eq!(result.username, "alice");
        assert_eq!(result.role, Some("DBA".to_string()));
    }

    #[test]
    fn test_drop_user_validator_new() {
        let validator = DropUserValidator::new();
        assert_eq!(validator.username, "");
        assert!(!validator.if_exists);
    }

    #[test]
    fn test_drop_user_validator_empty_username() {
        let mut validator = DropUserValidator::new();
        let stmt = DropUserStmt {
            span: Default::default(),
            username: "".to_string(),
            if_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_drop_user_validator_valid() {
        let mut validator = DropUserValidator::new();
        let stmt = DropUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            if_exists: true,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_drop_user_validator_unicode_username() {
        let mut validator = DropUserValidator::new();
        let stmt = DropUserStmt {
            span: Default::default(),
            username: "用户".to_string(),
            if_exists: false,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_alter_user_validator_new() {
        let validator = AlterUserValidator::new();
        assert_eq!(validator.username, "");
        assert_eq!(validator.password, None);
        assert_eq!(validator.new_role, None);
    }

    #[test]
    fn test_alter_user_validator_empty_username() {
        let mut validator = AlterUserValidator::new();
        let stmt = AlterUserStmt {
            span: Default::default(),
            username: "".to_string(),
            password: Some("newpass".to_string()),
            new_role: None,
            is_locked: None,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_alter_user_validator_valid_password_change() {
        let mut validator = AlterUserValidator::new();
        let stmt = AlterUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: Some("newpass123".to_string()),
            new_role: None,
            is_locked: None,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_alter_user_validator_valid_role_change() {
        let mut validator = AlterUserValidator::new();
        let stmt = AlterUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: None,
            new_role: Some("ADMIN".to_string()),
            is_locked: None,
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_alter_user_validator_invalid_role() {
        let mut validator = AlterUserValidator::new();
        let stmt = AlterUserStmt {
            span: Default::default(),
            username: "testuser".to_string(),
            password: None,
            new_role: Some("INVALID".to_string()),
            is_locked: None,
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_change_password_validator_new() {
        let validator = ChangePasswordValidator::new();
        assert_eq!(validator.username, None);
        assert_eq!(validator.old_password, "");
        assert_eq!(validator.new_password, "");
    }

    #[test]
    fn test_change_password_validator_empty_passwords() {
        let mut validator = ChangePasswordValidator::new();
        let stmt = ChangePasswordStmt {
            span: Default::default(),
            username: Some("testuser".to_string()),
            old_password: "".to_string(),
            new_password: "newpass".to_string(),
        };
        assert!(validator.validate_impl(&stmt).is_err());
    }

    #[test]
    fn test_change_password_validator_valid() {
        let mut validator = ChangePasswordValidator::new();
        let stmt = ChangePasswordStmt {
            span: Default::default(),
            username: Some("testuser".to_string()),
            old_password: "oldpass".to_string(),
            new_password: "newpass".to_string(),
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }

    #[test]
    fn test_change_password_validator_unicode_passwords() {
        let mut validator = ChangePasswordValidator::new();
        let stmt = ChangePasswordStmt {
            span: Default::default(),
            username: Some("testuser".to_string()),
            old_password: "旧密码123".to_string(),
            new_password: "新密码456".to_string(),
        };
        assert!(validator.validate_impl(&stmt).is_ok());
    }
}
