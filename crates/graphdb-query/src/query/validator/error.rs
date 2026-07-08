//! Validating Error Types
//!
//! Covers errors related to query validation and Schema validation

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

use crate::core::error::query::QueryError;

/// Validation error type enumeration (harmonized version)
///
/// Provides complete validation error categorization, corresponding to QueryError::InvalidQuery
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValidationErrorType {
    SyntaxError,
    SemanticError,
    TypeError,
    TypeMismatch,
    AliasError,
    AggregateError,
    PaginationError,
    ExpressionDepthError,
    VariableNotFound,
    CyclicReference,
    DivisionByZero,
    TooManyArguments,
    TooManyElements,
    DuplicateKey,
    ConstraintViolation,
}

impl fmt::Display for ValidationErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationErrorType::SyntaxError => write!(f, "Syntax error"),
            ValidationErrorType::SemanticError => write!(f, "Semantic error"),
            ValidationErrorType::TypeError => write!(f, "Type error"),
            ValidationErrorType::TypeMismatch => write!(f, "Type mismatch"),
            ValidationErrorType::AliasError => write!(f, "Alias error"),
            ValidationErrorType::AggregateError => write!(f, "Aggregate error"),
            ValidationErrorType::PaginationError => write!(f, "Pagination error"),
            ValidationErrorType::ExpressionDepthError => write!(f, "Expression depth error"),
            ValidationErrorType::VariableNotFound => write!(f, "Variable not found"),
            ValidationErrorType::CyclicReference => write!(f, "Cyclic reference"),
            ValidationErrorType::DivisionByZero => write!(f, "Division by zero"),
            ValidationErrorType::TooManyArguments => write!(f, "Too many arguments"),
            ValidationErrorType::TooManyElements => write!(f, "Too many elements"),
            ValidationErrorType::DuplicateKey => write!(f, "Duplicate key"),
            ValidationErrorType::ConstraintViolation => write!(f, "Constraint violation"),
        }
    }
}

impl From<ValidationErrorType> for QueryError {
    fn from(e: ValidationErrorType) -> Self {
        match e {
            ValidationErrorType::SyntaxError => QueryError::parse_error(e.to_string()),
            _ => QueryError::invalid_query(e.to_string()),
        }
    }
}

/// Unified validation error structure
///
/// Contains error type, error message and location information
/// Serialization/deserialization support for cross-module delivery
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationError {
    pub message: String,
    pub error_type: ValidationErrorType,
    pub context: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>, error_type: ValidationErrorType) -> Self {
        Self {
            message: message.into(),
            error_type,
            context: None,
            line: None,
            column: None,
        }
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    pub fn at_position(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error_type, self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Schema Validation Error Types
#[derive(Error, Debug, Clone, PartialEq)]
pub enum SchemaValidationError {
    #[error("Schema not found: {0}")]
    SchemaNotFound(String),

    #[error("Invalid schema definition: {0}")]
    InvalidSchema(String),

    #[error("Property type error: {0}")]
    PropertyTypeError(String),

    #[error("Required property missing: {0}")]
    RequiredPropertyMissing(String),

    #[error("Property validation failed: {0}")]
    PropertyValidationFailed(String),

    #[error("Schema conflict: {0}")]
    SchemaConflict(String),

    #[error("Unsupported schema operation: {0}")]
    UnsupportedOperation(String),
}

impl From<ValidationError> for crate::core::error::DBError {
    fn from(e: ValidationError) -> Self {
        let msg = e.to_string();
        crate::core::error::DBError::validation(msg).with_source(Box::new(e))
    }
}

/// Schema Validation Results
#[derive(Debug, Clone)]
pub struct SchemaValidationResult {
    pub is_valid: bool,
    pub errors: Vec<SchemaValidationError>,
}

impl SchemaValidationResult {
    pub fn success() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
        }
    }

    pub fn failure(errors: Vec<SchemaValidationError>) -> Self {
        Self {
            is_valid: false,
            errors,
        }
    }

    pub fn add_error(&mut self, error: SchemaValidationError) {
        self.is_valid = false;
        self.errors.push(error);
    }
}
