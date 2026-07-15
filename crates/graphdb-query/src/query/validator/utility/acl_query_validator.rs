use crate::query::parser::ast::stmt::{Ast, DescribeUserStmt, ShowRolesStmt, ShowUsersStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

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

        if self.username.is_empty() {
            return Err(ValidationError::new(
                "Username cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }
}

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
