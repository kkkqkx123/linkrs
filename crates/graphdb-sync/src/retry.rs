//! Retry utility for sync operations
//!
//! Provides retry logic for handling transient failures in index synchronization.

use std::time::Duration;

use tokio::time::sleep;

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Backoff multiplier (e.g., 2.0 means exponential backoff doubles each time)
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        }
    }
}

/// Default retry configuration for local (fulltext) operations
pub fn default_local_retry_config() -> RetryConfig {
    RetryConfig::new(2, Duration::from_millis(10), Duration::from_millis(500))
}

/// Default retry configuration for remote (vector) operations
pub fn default_remote_retry_config() -> RetryConfig {
    RetryConfig::new(3, Duration::from_millis(50), Duration::from_secs(5))
}

impl RetryConfig {
    pub fn new(max_retries: u32, initial_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_retries,
            initial_delay,
            max_delay,
            backoff_multiplier: 2.0,
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_initial_delay(mut self, initial_delay: Duration) -> Self {
        self.initial_delay = initial_delay;
        self
    }

    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }
}

/// Retry result
#[derive(Debug)]
pub enum RetryResult<T, E> {
    /// Operation succeeded
    Success(T),
    /// Operation failed after all retries
    Failed(E),
    /// Operation should not be retried
    DoNotRetry(E),
}

/// Execute a function with retry logic
pub async fn with_retry<F, Fut, T, E>(func: F, config: &RetryConfig) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut delay = config.initial_delay;
    let mut last_error: Option<E> = None;

    for attempt in 0..=config.max_retries {
        match func().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);

                if attempt < config.max_retries {
                    log::warn!(
                        "Operation failed (attempt {}), retrying in {:?}...",
                        attempt + 1,
                        delay
                    );
                    sleep(delay).await;

                    // Exponential backoff with max delay cap
                    delay =
                        std::cmp::min(delay.mul_f64(config.backoff_multiplier), config.max_delay);
                }
            }
        }
    }

    Err(last_error.unwrap())
}

/// Execute a function with retry logic and custom error handling
pub async fn with_retry_and_handler<F, Fut, T, E, H>(
    func: F,
    config: &RetryConfig,
    error_handler: H,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug + Clone,
    H: Fn(&E, u32) -> bool, // Returns true if should retry
{
    let mut delay = config.initial_delay;
    let mut last_error: Option<E> = None;

    for attempt in 0..=config.max_retries {
        match func().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Check if error handler allows retry
                if !error_handler(&e, attempt + 1) {
                    return Err(e);
                }

                last_error = Some(e);

                if attempt < config.max_retries {
                    log::warn!(
                        "Operation failed (attempt {}), retrying in {:?}...",
                        attempt + 1,
                        delay
                    );
                    sleep(delay).await;

                    // Exponential backoff with max delay cap
                    delay =
                        std::cmp::min(delay.mul_f64(config.backoff_multiplier), config.max_delay);
                }
            }
        }
    }

    Err(last_error.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_retry_success_on_first_attempt() {
        let config = RetryConfig::default();
        let attempts = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let mut count = attempts.lock().await;
                    *count += 1;
                    Ok::<_, String>("success".to_string())
                }
            },
            &config,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(*attempts.lock().await, 1);
    }

    #[tokio::test]
    async fn test_retry_eventual_success() {
        let config = RetryConfig::default();
        let attempts = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let mut count = attempts.lock().await;
                    *count += 1;
                    if *count < 3 {
                        Err::<String, _>("temporary error".to_string())
                    } else {
                        Ok::<_, String>("success".to_string())
                    }
                }
            },
            &config,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(*attempts.lock().await, 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig::default().with_max_retries(2);
        let attempts = std::sync::Arc::new(tokio::sync::Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result = with_retry(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let mut count = attempts.lock().await;
                    *count += 1;
                    Err::<String, _>("persistent error".to_string())
                }
            },
            &config,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(*attempts.lock().await, 3); // Initial + 2 retries
    }
}
