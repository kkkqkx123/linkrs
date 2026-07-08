//! Permission control class statement validator
//! Corresponding to the functionality of NebulaGraph ACLValidator
//! Verify statements related to privilege management, such as CREATE USER, DROP USER, ALTER USER, GRANT, and REVOKE.
//!
//! Design principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All statements related to permission classes are global in nature; there is no need to pre-select a specific scope (i.e., a specific “space” in the programming context) for them.
//! 3. Verify the user's existence and the legitimacy of their role.

use crate::query::parser::ast::stmt::{
    AlterUserStmt, Ast, ChangePasswordStmt, CreateUserStmt, DescribeUserStmt, DropUserStmt,
    GrantStmt, RevokeStmt, RoleType, ShowRolesStmt, ShowUsersStmt,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified user information
#[derive(Debug, Clone)]
pub struct ValidatedUser {
    pub username: String,
    pub role: Option<String>,
}

/// CREATE USER statement validator
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

        // Verify that the username is not empty.
        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // The password must not be empty.
        if self.password.is_empty() {
            return Err(ValidationError::new(
                "Password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify the legitimacy of the role
        if let Some(ref role) = self.role {
            Self::validate_role(role)?;
        }

        Ok(())
    }

    fn validate_role(role: &str) -> Result<(), ValidationError> {
        match role.to_uppercase().as_str() {
            "GOD" | "ADMIN" | "DBA" | "USER" | "GUEST" => Ok(()),
            _ => Err(ValidationError::new(
                format!("Invalid role: {}", role),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    pub fn validated_result(&self) -> ValidatedUser {
        ValidatedUser {
            username: self.username.clone(),
            role: self.role.clone(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method takes `Arc<Ast>` and `Arc<QueryContext>` as arguments.
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

/// DROP USER statement validator
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

        // Verify that the username is not empty.
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

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
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

/// ALTER USER statement validator
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

        // Verify that the username is not empty.
        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that there is at least one modification.
        if self.password.is_none() && self.new_role.is_none() && self.is_locked.is_none() {
            return Err(ValidationError::new(
                "At least one modification is required".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify the legitimacy of the role
        if let Some(ref role) = self.new_role {
            CreateUserValidator::validate_role(role)?;
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

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
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

/// CHANGE PASSWORD statement validator
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

        // Verify that the old password is not empty.
        if self.old_password.is_empty() {
            return Err(ValidationError::new(
                "Old password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the new password is not empty.
        if self.new_password.is_empty() {
            return Err(ValidationError::new(
                "New password cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the new password is different from the old one.
        if self.old_password == self.new_password {
            return Err(ValidationError::new(
                "New password must be different from old password".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
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

/// Verified permission information
#[derive(Debug, Clone)]
pub struct ValidatedGrant {
    pub role: RoleType,
    pub space_name: String,
    pub username: String,
}

/// The GRANT statement validator
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

        // Verify that the space name is not empty.
        if self.space_name.is_empty() {
            return Err(ValidationError::new(
                "Space name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the username is not empty.
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

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
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

/// REVOKE statement validator
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

        // Verify that the space name is not empty.
        if self.space_name.is_empty() {
            return Err(ValidationError::new(
                "Space name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the username is not empty.
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

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
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

/// “DESCRIBE USER” statement validator
#[derive(Debug)]
pub struct DescribeUserValidator {
    username: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl DescribeUserValidator {
    pub fn new() -> Self {
        Self {
            username: String::new(),
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "User".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Roles".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &DescribeUserStmt) -> Result<(), ValidationError> {
        self.username = stmt.username.clone();

        // Verify that the username is not empty.
        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for DescribeUserValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let describe_user_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::DescribeUser(describe_user_stmt) => describe_user_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected DESCRIBE USER statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(describe_user_stmt)?;

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

impl Default for DescribeUserValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// SHOW USERS Statement Validator
#[derive(Debug)]
pub struct ShowUsersValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowUsersValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Account".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, _stmt: &ShowUsersStmt) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for ShowUsersValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_users_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowUsers(show_users_stmt) => show_users_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW USERS statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_users_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("ShowUsers".to_string());

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

impl Default for ShowUsersValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// SHOW ROLES statement validator
#[derive(Debug)]
pub struct ShowRolesValidator {
    space_name: Option<String>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowRolesValidator {
    pub fn new() -> Self {
        Self {
            space_name: None,
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "Account".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Role".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &ShowRolesStmt) -> Result<(), ValidationError> {
        self.space_name = stmt.space_name.clone();
        Ok(())
    }
}

impl StatementValidator for ShowRolesValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_roles_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowRoles(s) => s,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW ROLES statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_roles_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("ShowRoles".to_string());

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

impl Default for ShowRolesValidator {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Unit Tests ====================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== CreateUserValidator Tests ====================

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
        assert!(CreateUserValidator::validate_role("GOD").is_ok());
        assert!(CreateUserValidator::validate_role("ADMIN").is_ok());
        assert!(CreateUserValidator::validate_role("DBA").is_ok());
        assert!(CreateUserValidator::validate_role("USER").is_ok());
        assert!(CreateUserValidator::validate_role("GUEST").is_ok());
    }

    #[test]
    fn test_validate_role_case_insensitive() {
        assert!(CreateUserValidator::validate_role("god").is_ok());
        assert!(CreateUserValidator::validate_role("Admin").is_ok());
        assert!(CreateUserValidator::validate_role("dba").is_ok());
    }

    #[test]
    fn test_validate_role_invalid() {
        assert!(CreateUserValidator::validate_role("SUPERUSER").is_err());
        assert!(CreateUserValidator::validate_role("ROOT").is_err());
        assert!(CreateUserValidator::validate_role("INVALID").is_err());
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

    // ==================== DropUserValidator Tests ====================

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

    // ==================== AlterUserValidator Tests ====================

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

    // ==================== ChangePasswordValidator Tests ====================

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

    // ==================== GrantValidator Tests ====================

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

    // ==================== RevokeValidator Tests ====================

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
