//! Definition of external error codes
//!
//! This module defines a standardized error code system for the following purposes:
//! Client response
//! The API returns the data.
//! Protocol serialization
//!
//! Error code format: XXYY
//! XX: Error category (00 = Success, 01 = Syntax, 02 = Execution, 03 = Validation, 04 = Permissions, 05 = Resources, 09 = System)
//! - YY: Specific error

use serde::{Deserialize, Serialize};

/// External error code – used for client responses
///
/// Design principles:
/// Stability: Error codes should not be modified arbitrarily once they have been defined, in order to ensure compatibility with clients.
/// 2. Simplification: Only the necessary error information is displayed, without any details about the internal implementation.
/// 3. Standardization: Adherence to common error code design specifications such as those for HTTP/GraphQL
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ErrorCode {
    // ==================== Success (00xx) ====================
    #[default]
    Success = 0,

    // ==================== Grammar error (01xx) ====================
    /// Common grammar errors
    SyntaxError = 100,
    /// Analysis error
    ParseError = 101,
    /// Invalid statement.
    InvalidStatement = 102,
    /// The necessary parameters are missing.
    MissingParameter = 103,

    // ==================== Execution error (02xx) ====================
    /// General Execution Error
    ExecutionError = 200,
    /// The operation timed out.
    Timeout = 201,
    /// Insufficient resources
    ResourceExhausted = 202,
    /// Concurrency conflicts
    Conflict = 203,
    /// Deadlock detection
    Deadlock = 204,

    // Verification error (03xx)
    /// General Verification Error
    ValidationError = 300,
    /// Type error
    TypeError = 301,
    /// Invalid input.
    InvalidInput = 302,
    /// Constraint violation
    ConstraintViolation = 303,

    // ==================== Permission error (04xx) ====================
    /// Insufficient permissions
    PermissionDenied = 400,
    /// Unauthenticated
    Unauthorized = 401,
    /// Access prohibited.
    Forbidden = 403,

    // Resource error (05xx)
    /// The resource was not found.
    ResourceNotFound = 500,
    /// The resource already exists.
    ResourceAlreadyExists = 501,
    /// The resource is not available.
    ResourceUnavailable = 502,

    // ==================== System Error (09xx) ====================
    /// Internal server error
    InternalError = 900,
    /// The service is not available.
    ServiceUnavailable = 901,
    /// Unknown error
    Unknown = 999,
}

impl ErrorCode {
    /// Obtain the i32 value of the error code
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    /// Retrieve the error code based on the i32 value.
    pub fn from_i32(code: i32) -> Option<Self> {
        match code {
            0 => Some(ErrorCode::Success),
            100 => Some(ErrorCode::SyntaxError),
            101 => Some(ErrorCode::ParseError),
            102 => Some(ErrorCode::InvalidStatement),
            103 => Some(ErrorCode::MissingParameter),
            200 => Some(ErrorCode::ExecutionError),
            201 => Some(ErrorCode::Timeout),
            202 => Some(ErrorCode::ResourceExhausted),
            203 => Some(ErrorCode::Conflict),
            204 => Some(ErrorCode::Deadlock),
            300 => Some(ErrorCode::ValidationError),
            301 => Some(ErrorCode::TypeError),
            302 => Some(ErrorCode::InvalidInput),
            303 => Some(ErrorCode::ConstraintViolation),
            400 => Some(ErrorCode::PermissionDenied),
            401 => Some(ErrorCode::Unauthorized),
            403 => Some(ErrorCode::Forbidden),
            500 => Some(ErrorCode::ResourceNotFound),
            501 => Some(ErrorCode::ResourceAlreadyExists),
            502 => Some(ErrorCode::ResourceUnavailable),
            900 => Some(ErrorCode::InternalError),
            901 => Some(ErrorCode::ServiceUnavailable),
            999 => Some(ErrorCode::Unknown),
            _ => None,
        }
    }

    /// Obtain the error category
    pub fn category(&self) -> ErrorCategory {
        match self.as_i32() {
            0 => ErrorCategory::Success,
            100..=199 => ErrorCategory::Syntax,
            200..=299 => ErrorCategory::Execution,
            300..=399 => ErrorCategory::Validation,
            400..=499 => ErrorCategory::Permission,
            500..=599 => ErrorCategory::Resource,
            900..=999 => ErrorCategory::System,
            _ => ErrorCategory::Unknown,
        }
    }

    /// Retrieve the default error message.
    pub fn default_message(&self) -> &'static str {
        match self {
            ErrorCode::Success => "success",
            ErrorCode::SyntaxError => "syntax error",
            ErrorCode::ParseError => "parse error",
            ErrorCode::InvalidStatement => "invalid statement",
            ErrorCode::MissingParameter => "missing required parameter",
            ErrorCode::ExecutionError => "execution error",
            ErrorCode::Timeout => "execution timeout",
            ErrorCode::ResourceExhausted => "resource exhausted",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Deadlock => "deadlock detected",
            ErrorCode::ValidationError => "validation error",
            ErrorCode::TypeError => "type error",
            ErrorCode::InvalidInput => "invalid input",
            ErrorCode::ConstraintViolation => "constraint violation",
            ErrorCode::PermissionDenied => "permission denied",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::ResourceNotFound => "resource not found",
            ErrorCode::ResourceAlreadyExists => "resource already exists",
            ErrorCode::ResourceUnavailable => "resource unavailable",
            ErrorCode::InternalError => "internal server error",
            ErrorCode::ServiceUnavailable => "service unavailable",
            ErrorCode::Unknown => "unknown error",
        }
    }

    /// Determine whether it represents a successful state.
    pub fn is_success(&self) -> bool {
        matches!(self, ErrorCode::Success)
    }

    /// Determine whether it is a client-side error (a 4xx error).
    pub fn is_client_error(&self) -> bool {
        let code = self.as_i32();
        (100..=499).contains(&code)
    }

    /// Determine whether it is a server error (errors of the 5xx/9xx type).
    pub fn is_server_error(&self) -> bool {
        let code = self.as_i32();
        (500..=599).contains(&code) || (900..=999).contains(&code)
    }

    /// Determine whether a failed attempt can be retried.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ErrorCode::Timeout
                | ErrorCode::Conflict
                | ErrorCode::Deadlock
                | ErrorCode::ResourceExhausted
                | ErrorCode::ServiceUnavailable
        )
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.as_i32(), self.default_message())
    }
}

/// error category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    Success,
    Syntax,
    Execution,
    Validation,
    Permission,
    Resource,
    System,
    Unknown,
}

impl ErrorCategory {
    /// Gets the HTTP status code mapping of the category
    pub fn to_http_status(&self) -> u16 {
        match self {
            ErrorCategory::Success => 200,
            ErrorCategory::Syntax => 400,
            ErrorCategory::Execution => 500,
            ErrorCategory::Validation => 422,
            ErrorCategory::Permission => 403,
            ErrorCategory::Resource => 404,
            ErrorCategory::System => 500,
            ErrorCategory::Unknown => 500,
        }
    }
}

/// External error message-used to serialize into the response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicError {
    /// error code
    pub code: ErrorCode,
    /// error message
    pub message: String,
}

impl PublicError {
    /// Create new external errors
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Create errors using default messages
    pub fn with_default_message(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.default_message().to_string(),
        }
    }

    /// Create a successful response
    pub fn success() -> Self {
        Self {
            code: ErrorCode::Success,
            message: "successes".to_string(),
        }
    }
}

/// conversion trait of internal errors to external errors
///
/// Implementing this trait can convert internal errors into external errors and filter sensitive information
pub trait ToPublicError {
    /// Convert to external error
    fn to_public_error(&self) -> PublicError;

    /// Get external error codes
    fn to_error_code(&self) -> ErrorCode;

    /// Get external error messages (filter sensitive information)
    fn to_public_message(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_as_i32() {
        assert_eq!(ErrorCode::Success.as_i32(), 0);
        assert_eq!(ErrorCode::SyntaxError.as_i32(), 100);
        assert_eq!(ErrorCode::InternalError.as_i32(), 900);
    }

    #[test]
    fn test_error_code_from_i32() {
        assert_eq!(ErrorCode::from_i32(0), Some(ErrorCode::Success));
        assert_eq!(ErrorCode::from_i32(100), Some(ErrorCode::SyntaxError));
        assert_eq!(ErrorCode::from_i32(999), Some(ErrorCode::Unknown));
        assert_eq!(ErrorCode::from_i32(12345), None);
    }

    #[test]
    fn test_error_code_category() {
        assert_eq!(ErrorCode::Success.category(), ErrorCategory::Success);
        assert_eq!(ErrorCode::SyntaxError.category(), ErrorCategory::Syntax);
        assert_eq!(
            ErrorCode::ExecutionError.category(),
            ErrorCategory::Execution
        );
        assert_eq!(ErrorCode::InternalError.category(), ErrorCategory::System);
    }

    #[test]
    fn test_error_code_is_success() {
        assert!(ErrorCode::Success.is_success());
        assert!(!ErrorCode::SyntaxError.is_success());
        assert!(!ErrorCode::InternalError.is_success());
    }

    #[test]
    fn test_error_code_is_retryable() {
        assert!(ErrorCode::Timeout.is_retryable());
        assert!(ErrorCode::Conflict.is_retryable());
        assert!(ErrorCode::Deadlock.is_retryable());
        assert!(!ErrorCode::SyntaxError.is_retryable());
        assert!(!ErrorCode::PermissionDenied.is_retryable());
    }

    #[test]
    fn test_public_error() {
        let err = PublicError::new(
            ErrorCode::ResourceNotFound,
            "user does not exist".to_string(),
        );
        assert_eq!(err.code, ErrorCode::ResourceNotFound);
        assert_eq!(err.message, "user does not exist");

        let default_err = PublicError::with_default_message(ErrorCode::Timeout);
        assert_eq!(default_err.code, ErrorCode::Timeout);
        assert_eq!(default_err.message, "execution timeout");
    }
}
