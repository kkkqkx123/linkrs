//! Authentication Error Type
//!
//! Covers user authentication related errors, including login, password validation, etc.

use thiserror::Error;

use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};

/// Authentication operation result type alias
pub type AuthResult<T> = Result<T, AuthError>;

/// Authentication-related errors
#[derive(Error, Debug, Clone)]
pub enum AuthError {
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Username or password cannot be empty")]
    EmptyCredentials,

    #[error("Invalid username or password, {0} attempts remaining")]
    InvalidCredentials(u32),

    #[error("Maximum attempts exceeded")]
    MaxAttemptsExceeded,

    #[error("Authenticator error: {0}")]
    AuthenticatorError(String),
}

impl ToPublicError for AuthError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self {
            AuthError::AuthenticationFailed(_) => ErrorCode::Unauthorized,
            AuthError::EmptyCredentials => ErrorCode::InvalidInput,
            AuthError::InvalidCredentials(_) => ErrorCode::Unauthorized,
            AuthError::MaxAttemptsExceeded => ErrorCode::ResourceExhausted,
            AuthError::AuthenticatorError(_) => ErrorCode::InternalError,
        }
    }

    fn to_public_message(&self) -> String {
        self.to_string()
    }
}
