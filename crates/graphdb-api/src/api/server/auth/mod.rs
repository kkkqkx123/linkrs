//! Authentication module
//!
//! Provide user authentication and authorization features.

pub mod authenticator;
pub mod error;
pub use crate::core::UserStorage;
pub use authenticator::{Authenticator, AuthenticatorFactory, PasswordAuthenticator, UserVerifier};
pub use error::{AuthError, AuthResult};
