//! Transaction Statement Validator
//!
//! Simple validator for BEGIN, COMMIT, ROLLBACK transaction statements.

use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::ValidationError;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Transaction statement validator
#[derive(Debug, Clone)]
pub struct TransactionValidator {
    stmt_type: StatementType,
    expression_props: ExpressionProps,
}

impl TransactionValidator {
    pub fn new(stmt_type: StatementType) -> Self {
        Self {
            stmt_type,
            expression_props: ExpressionProps::default(),
        }
    }
}

impl StatementValidator for TransactionValidator {
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let info = ValidationInfo::new();
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        self.stmt_type
    }

    fn inputs(&self) -> &[ColumnDef] {
        &[]
    }

    fn outputs(&self) -> &[ColumnDef] {
        &[]
    }

    fn is_global_statement(&self) -> bool {
        true
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &[]
    }
}
