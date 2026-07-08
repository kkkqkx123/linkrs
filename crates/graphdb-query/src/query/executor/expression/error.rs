//! Expression Error Type
//!
//! Contains error type, error message and optional location information
//! Serialization/deserialization support for cross-module delivery

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Expression error (structured design)
#[derive(Error, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpressionError {
    /// Type of error
    pub error_type: ExpressionErrorType,
    /// error message
    pub message: String,
    /// error location
    pub position: Option<ExpressionPosition>,
}

/// Expression Error Type Enumeration
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExpressionErrorType {
    /// type error
    TypeError,
    /// undefined variable
    UndefinedVariable,
    /// undefined function
    UndefinedFunction,
    /// unknown function
    UnknownFunction,
    /// function error
    FunctionError,
    /// Wrong number of parameters
    ArgumentCountError,
    /// Number of invalid parameters
    InvalidArgumentCount,
    /// overflow error
    Overflow,
    /// indexing cross-border
    IndexOutOfBounds,
    /// null hypothesis
    NullError,
    /// grammatical error
    SyntaxError,
    /// Invalid operation
    InvalidOperation,
    /// Attribute not found
    PropertyNotFound,
    /// run-time error (in computing)
    RuntimeError,
    /// Unsupported operations
    UnsupportedOperation,
    /// type conversion error
    TypeConversionError,
    /// operator error
    OperatorError,
    /// Tag not found
    LabelNotFound,
    /// The edge is not found.
    EdgeNotFound,
    /// path error
    PathError,
    /// range error
    RangeError,
    /// Polymerization function error
    AggregateError,
    /// verification error
    ValidationError,
    /// function execution error
    FunctionExecutionError,
}

impl std::fmt::Display for ExpressionErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionErrorType::TypeError => write!(f, "Type error"),
            ExpressionErrorType::UndefinedVariable => write!(f, "Undefined variable"),
            ExpressionErrorType::UndefinedFunction => write!(f, "Undefined function"),
            ExpressionErrorType::UnknownFunction => write!(f, "Unknown function"),
            ExpressionErrorType::FunctionError => write!(f, "Function error"),
            ExpressionErrorType::ArgumentCountError => write!(f, "Argument count error"),
            ExpressionErrorType::InvalidArgumentCount => write!(f, "Invalid argument count"),
            ExpressionErrorType::Overflow => write!(f, "Overflow error"),
            ExpressionErrorType::IndexOutOfBounds => write!(f, "Index out of bounds"),
            ExpressionErrorType::NullError => write!(f, "Null error"),
            ExpressionErrorType::SyntaxError => write!(f, "Syntax error"),
            ExpressionErrorType::InvalidOperation => write!(f, "Invalid operation"),
            ExpressionErrorType::PropertyNotFound => write!(f, "Property not found"),
            ExpressionErrorType::RuntimeError => write!(f, "Runtime error"),
            ExpressionErrorType::UnsupportedOperation => write!(f, "Unsupported operation"),
            ExpressionErrorType::TypeConversionError => write!(f, "Type conversion error"),
            ExpressionErrorType::OperatorError => write!(f, "Operator error"),
            ExpressionErrorType::LabelNotFound => write!(f, "Label not found"),
            ExpressionErrorType::EdgeNotFound => write!(f, "Edge not found"),
            ExpressionErrorType::PathError => write!(f, "Path error"),
            ExpressionErrorType::RangeError => write!(f, "Range error"),
            ExpressionErrorType::AggregateError => write!(f, "Aggregate error"),
            ExpressionErrorType::ValidationError => write!(f, "Validation error"),
            ExpressionErrorType::FunctionExecutionError => write!(f, "Function execution error"),
        }
    }
}

/// Expression Error Location Information
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpressionPosition {
    /// line number
    pub line: usize,
    /// column number
    pub column: usize,
    /// offset
    pub offset: usize,
    /// lengths
    pub length: usize,
}

impl ExpressionError {
    /// Creating a new expression error
    pub fn new(error_type: ExpressionErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            position: None,
        }
    }

    /// Setting the error position
    pub fn with_position(
        mut self,
        line: usize,
        column: usize,
        offset: usize,
        length: usize,
    ) -> Self {
        self.position = Some(ExpressionPosition {
            line,
            column,
            offset,
            length,
        });
        self
    }

    /// Create Type Error
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::TypeError, message)
    }

    /// Create undefined variable error
    pub fn undefined_variable(name: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::UndefinedVariable,
            format!("Undefined variable: {}", name.into()),
        )
    }

    /// Create undefined function error
    pub fn undefined_function(name: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::UndefinedFunction,
            format!("Undefined function: {}", name.into()),
        )
    }

    /// Wrong number of creation parameters
    pub fn argument_count_error(expected: usize, actual: usize) -> Self {
        Self::new(
            ExpressionErrorType::ArgumentCountError,
            format!(
                "Argument count error: expected {}, got {}",
                expected, actual
            ),
        )
    }

    /// Create Overflow Error
    pub fn overflow(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::Overflow, message)
    }

    /// Create index out of bounds error
    pub fn index_out_of_bounds(index: isize, size: usize) -> Self {
        Self::new(
            ExpressionErrorType::IndexOutOfBounds,
            format!("Index out of bounds: index {}, size {}", index, size),
        )
    }

    /// Create Null Error
    pub fn null_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::NullError, message)
    }

    /// Creating Syntax Errors
    pub fn syntax_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::SyntaxError, message)
    }

    /// Creating Runtime Errors
    pub fn runtime_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::RuntimeError, message)
    }

    /// Create function error
    pub fn function_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::FunctionError, message)
    }

    /// Create invalid operation error
    pub fn invalid_operation(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::InvalidOperation, message)
    }

    /// Error: The attribute creation could not be found.
    pub fn property_not_found(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::PropertyNotFound, message)
    }

    /// Error creating an unknown function.
    pub fn unknown_function(name: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::UnknownFunction,
            format!("Unknown function: {}", name.into()),
        )
    }

    /// Error: The number of invalid parameters created is incorrect.
    pub fn invalid_argument_count(name: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::InvalidArgumentCount,
            format!("Invalid argument count: {}", name.into()),
        )
    }

    /// An error occurred during the creation of an unsupported operation.
    pub fn unsupported_operation(
        operation: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self::new(
            ExpressionErrorType::UnsupportedOperation,
            format!(
                "Unsupported operation: {}, suggestion: {}",
                operation.into(),
                suggestion.into()
            ),
        )
    }

    /// An error occurred during the creation of the type conversion.
    pub fn type_conversion_error(from_type: impl Into<String>, to_type: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::TypeConversionError,
            format!(
                "Type conversion error: cannot convert from {} to {}",
                from_type.into(),
                to_type.into()
            ),
        )
    }

    /// An error occurred while creating the operator.
    pub fn operator_error(operator: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::OperatorError,
            format!("Operator error: {}: {}", operator.into(), message.into()),
        )
    }

    /// An error occurred while trying to create the tags.
    pub fn label_not_found(label: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::LabelNotFound,
            format!("Label not found: {}", label.into()),
        )
    }

    /// Error: Edge creation not found.
    pub fn edge_not_found(edge: impl Into<String>) -> Self {
        Self::new(
            ExpressionErrorType::EdgeNotFound,
            format!("Edge not found: {}", edge.into()),
        )
    }

    /// An error occurred while creating the path.
    pub fn path_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::PathError, message)
    }

    /// Error creating the range.
    pub fn range_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::RangeError, message)
    }

    /// An error occurred while creating the aggregate function.
    pub fn aggregate_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::AggregateError, message)
    }

    /// Create a validation error.
    pub fn validation_error(message: impl Into<String>) -> Self {
        Self::new(ExpressionErrorType::ValidationError, message)
    }
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.error_type, self.message)
    }
}

impl From<ExpressionError> for crate::core::error::DBError {
    fn from(e: ExpressionError) -> Self {
        let msg = e.to_string();
        Self::expression(msg).with_source(Box::new(e))
    }
}
