use dashmap::DashMap;
use std::sync::Arc;

use super::AuthResult;
use crate::config::AuthConfig;

/// Authentication trait
pub trait Authenticator: Send + Sync {
    fn authenticate(&self, username: &str, password: &str) -> AuthResult<()>;
}

/// Login Failure Record
#[derive(Debug, Clone)]
struct LoginAttempt {
    /// Remaining attempts
    remaining_attempts: u32,
}

/// User Authentication Callback Function Types
pub type UserVerifier = Arc<dyn Fn(&str, &str) -> AuthResult<bool> + Send + Sync>;

/// Password Authenticator - Supports failed login restrictions and account lockout
pub struct PasswordAuthenticator {
    /// User Authentication Callbacks
    user_verifier: UserVerifier,
    config: AuthConfig,
    /// Logging of user login attempts - using DashMap for high-performance concurrent access
    login_attempts: Arc<DashMap<String, LoginAttempt>>,
}

impl PasswordAuthenticator {
    pub fn new<F>(user_verifier: F, config: AuthConfig) -> Self
    where
        F: Fn(&str, &str) -> AuthResult<bool> + Send + Sync + 'static,
    {
        Self {
            user_verifier: Arc::new(user_verifier),
            config,
            login_attempts: Arc::new(DashMap::new()),
        }
    }

    /// Create default password authenticator (supports configured username and password)
    pub fn new_default(config: AuthConfig) -> Self {
        let default_username = config.default_username.clone();
        let default_password = config.default_password.clone();

        Self::new(
            move |username: &str, password: &str| {
                // Use the configured default username and password
                if username == default_username && password == default_password {
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
            config,
        )
    }

    /// Record Login Failure
    fn record_failed_attempt(&self, username: &str) {
        // If the login failure restriction is not enabled, it returns directly
        if self.config.failed_login_attempts == 0 {
            return;
        }

        let username_key = username.to_string();
        self.login_attempts
            .entry(username_key.clone())
            .or_insert(LoginAttempt {
                remaining_attempts: self.config.failed_login_attempts,
            });

        // Reduce the number of remaining attempts
        if let Some(mut attempt) = self.login_attempts.get_mut(&username_key) {
            if attempt.remaining_attempts > 0 {
                attempt.remaining_attempts -= 1;
            }
        }
    }

    /// Reset logging of login attempts (called on successful login)
    fn reset_attempts(&self, username: &str) {
        self.login_attempts.remove(username);
    }

    /// Verify user passwords
    fn verify_password(&self, username: &str, password: &str) -> AuthResult<bool> {
        (self.user_verifier)(username, password)
    }
}

impl Authenticator for PasswordAuthenticator {
    fn authenticate(&self, username: &str, password: &str) -> AuthResult<()> {
        use crate::api::server::auth::AuthError;

        // Check if authorization is enabled
        if !self.config.enable_authorize {
            return Ok(());
        }

        if username.is_empty() || password.is_empty() {
            return Err(AuthError::EmptyCredentials);
        }

        // Verify Password
        match self.verify_password(username, password) {
            Ok(true) => {
                // Login successful, reset attempt log
                self.reset_attempts(username);
                Ok(())
            }
            Ok(false) => {
                // Login failed, logging attempt
                self.record_failed_attempt(username);

                let username_key = username.to_string();
                if let Some(attempt) = self.login_attempts.get(&username_key) {
                    if attempt.remaining_attempts > 0 {
                        return Err(AuthError::InvalidCredentials(attempt.remaining_attempts));
                    } else {
                        return Err(AuthError::MaxAttemptsExceeded);
                    }
                }

                Err(AuthError::InvalidCredentials(0))
            }
            Err(e) => Err(e),
        }
    }
}

/// Authenticator Factory
pub struct AuthenticatorFactory;

impl AuthenticatorFactory {
    /// Creating a Password Authenticator
    pub fn create<F>(config: &AuthConfig, user_verifier: F) -> PasswordAuthenticator
    where
        F: Fn(&str, &str) -> AuthResult<bool> + Send + Sync + 'static,
    {
        PasswordAuthenticator::new(user_verifier, config.clone())
    }

    /// Creating a default password authenticator
    pub fn create_default(config: &AuthConfig) -> PasswordAuthenticator {
        PasswordAuthenticator::new_default(config.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> AuthConfig {
        AuthConfig {
            enable_authorize: true,
            failed_login_attempts: 3,
            session_idle_timeout_secs: 3600,
            default_username: "test".to_string(),
            default_password: "test123".to_string(),
            force_change_default_password: true,
        }
    }

    #[test]
    fn test_password_authenticator_success() {
        let config = create_test_config();
        let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(true), config);

        assert!(auth.authenticate("user", "pass").is_ok());
    }

    #[test]
    fn test_password_authenticator_failure() {
        let config = create_test_config();
        let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(false), config);

        assert!(auth.authenticate("user", "wrong_pass").is_err());
    }

    #[test]
    fn test_password_authenticator_default() {
        let config = AuthConfig {
            enable_authorize: true,
            failed_login_attempts: 0, // Disable Login Restrictions
            session_idle_timeout_secs: 3600,
            default_username: "admin".to_string(),
            default_password: "admin123".to_string(),
            force_change_default_password: false,
        };

        let auth = PasswordAuthenticator::new_default(config);

        // Use the correct default credentials
        assert!(auth.authenticate("admin", "admin123").is_ok());
        // Using the wrong credentials
        assert!(auth.authenticate("admin", "wrong").is_err());
        assert!(auth.authenticate("user", "admin123").is_err());
    }

    #[test]
    fn test_login_attempt_limit() {
        let config = AuthConfig {
            enable_authorize: true,
            failed_login_attempts: 2, // Maximum 2 failures
            session_idle_timeout_secs: 3600,
            default_username: "test".to_string(),
            default_password: "test123".to_string(),
            force_change_default_password: false,
        };

        let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(false), config);

        // First failure - 1 to go
        let result1 = auth.authenticate("user", "wrong");
        assert!(result1.is_err());
        assert!(result1
            .unwrap_err()
            .to_string()
            .contains("1 attempts remaining"));

        // Second failure - maximum number of attempts reached
        let result2 = auth.authenticate("user", "wrong");
        assert!(result2.is_err());
        assert!(result2
            .unwrap_err()
            .to_string()
            .contains("Maximum attempts exceeded"));

        // Third Failure - still showing that the maximum number of attempts has been reached
        let result3 = auth.authenticate("user", "wrong");
        assert!(result3.is_err());
        assert!(result3
            .unwrap_err()
            .to_string()
            .contains("Maximum attempts exceeded"));
    }

    #[test]
    fn test_successful_login_resets_attempts() {
        // Using Arc<AtomicBool> to share mutable state in a closure
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let config = AuthConfig {
            enable_authorize: true,
            failed_login_attempts: 2,
            session_idle_timeout_secs: 3600,
            default_username: "test".to_string(),
            default_password: "test123".to_string(),
            force_change_default_password: false,
        };

        let success = Arc::new(AtomicBool::new(false));
        let success_clone = success.clone();

        let auth = PasswordAuthenticator::new(
            move |_username: &str, _password: &str| {
                if success_clone.load(Ordering::SeqCst) {
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
            config,
        );

        // First failure.
        assert!(auth.authenticate("user", "wrong").is_err());

        // Successful logins should reset the failure count
        success.store(true, Ordering::SeqCst);
        assert!(auth.authenticate("user", "correct").is_ok());

        // Failed again and should be recounted
        success.store(false, Ordering::SeqCst);
        assert!(auth.authenticate("user", "wrong").is_err());
        // One more chance.
        assert!(auth.authenticate("user", "wrong").is_err());
    }

    #[test]
    fn test_authenticator_factory() {
        let config = AuthConfig {
            enable_authorize: true,
            failed_login_attempts: 0,
            session_idle_timeout_secs: 3600,
            default_username: "test".to_string(),
            default_password: "test123".to_string(),
            force_change_default_password: false,
        };

        let _auth =
            AuthenticatorFactory::create(&config, |_username: &str, _password: &str| Ok(true));
        // Verify successful creation (no longer return Result, just create successfully)

        let _auth_default = AuthenticatorFactory::create_default(&config);
        // Verify successful creation
    }
}
