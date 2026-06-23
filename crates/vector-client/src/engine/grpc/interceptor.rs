use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::Request;
use tracing::{debug, error, info, warn};

use crate::error::{Result, VectorClientError};

#[derive(Clone)]
pub struct GrpcInterceptor {
    api_key: Option<String>,
    request_counter: Arc<AtomicU64>,
    enable_logging: bool,
    enable_metrics: bool,
}

impl GrpcInterceptor {
    pub fn new(api_key: Option<String>, enable_logging: bool, enable_metrics: bool) -> Self {
        Self {
            api_key,
            request_counter: Arc::new(AtomicU64::new(0)),
            enable_logging,
            enable_metrics,
        }
    }

    pub fn with_api_key(api_key: String) -> Self {
        Self::new(Some(api_key), true, true)
    }

    pub fn without_auth() -> Self {
        Self::new(None, true, true)
    }

    pub fn request_count(&self) -> u64 {
        self.request_counter.load(Ordering::Relaxed)
    }

    pub fn apply_to_channel(&self, channel: Channel) -> Channel {
        channel
    }

    pub fn intercept<T>(&self, mut request: Request<T>) -> Request<T> {
        self.request_counter.fetch_add(1, Ordering::Relaxed);

        if let Some(ref key) = self.api_key {
            if let Ok(value) = MetadataValue::try_from(key) {
                request.metadata_mut().insert("api-key", value);
            } else {
                warn!("Skipping invalid API key metadata value");
            }
        }

        if self.enable_logging {
            let method = request
                .metadata()
                .get("grpc-method")
                .map(|v| v.to_str().unwrap_or("unknown"))
                .unwrap_or("unknown");
            let counter = self.request_counter.load(Ordering::Relaxed);
            info!(%method, %counter, "gRPC request");
        }

        request
    }

    pub fn log_response(&self, status: &str, duration: std::time::Duration, success: bool) {
        if self.enable_metrics {
            if success {
                debug!(latency_ms = duration.as_millis(), status, "gRPC success");
            } else {
                error!(latency_ms = duration.as_millis(), status, "gRPC error");
            }
        }
    }
}

#[derive(Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
            backoff_multiplier: 2.0,
        }
    }
}

#[derive(Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub timeout: std::time::Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: std::time::Duration::from_secs(60),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<tokio::sync::Mutex<CircuitBreakerState>>,
    failure_count: Arc<AtomicU64>,
    success_count: Arc<AtomicU64>,
    last_failure_time: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(tokio::sync::Mutex::new(CircuitBreakerState::Closed)),
            failure_count: Arc::new(AtomicU64::new(0)),
            success_count: Arc::new(AtomicU64::new(0)),
            last_failure_time: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn is_available(&self) -> bool {
        let state = self.state.lock().await;
        match *state {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => {
                let last_failure = self.last_failure_time.lock().await;
                if let Some(time) = *last_failure {
                    if time.elapsed() >= self.config.timeout {
                        drop(state);
                        let mut state = self.state.lock().await;
                        *state = CircuitBreakerState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitBreakerState::HalfOpen => true,
        }
    }

    pub async fn record_success(&self) {
        let state = self.state.lock().await;
        match *state {
            CircuitBreakerState::HalfOpen => {
                self.success_count.fetch_add(1, Ordering::Relaxed);
                if self.success_count.load(Ordering::Relaxed)
                    >= self.config.success_threshold as u64
                {
                    drop(state);
                    let mut state = self.state.lock().await;
                    *state = CircuitBreakerState::Closed;
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.success_count.store(0, Ordering::Relaxed);
                }
            }
            CircuitBreakerState::Closed => {
                self.failure_count.store(0, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub async fn record_failure(&self) {
        self.failure_count.fetch_add(1, Ordering::Relaxed);
        let mut last_failure = self.last_failure_time.lock().await;
        *last_failure = Some(Instant::now());

        if self.failure_count.load(Ordering::Relaxed) >= self.config.failure_threshold as u64 {
            let mut state = self.state.lock().await;
            *state = CircuitBreakerState::Open;
            self.success_count.store(0, Ordering::Relaxed);
        }
    }
}

pub struct GrpcClientConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub timeout: std::time::Duration,
    pub enable_logging: bool,
    pub enable_metrics: bool,
    pub retry_config: Option<RetryConfig>,
    pub circuit_breaker_config: Option<CircuitBreakerConfig>,
}

impl Default for GrpcClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:6333".to_string(),
            api_key: None,
            timeout: std::time::Duration::from_secs(30),
            enable_logging: true,
            enable_metrics: true,
            retry_config: Some(RetryConfig::default()),
            circuit_breaker_config: Some(CircuitBreakerConfig::default()),
        }
    }
}

pub async fn execute_with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;
    let mut backoff_ms = config.initial_backoff_ms;

    for attempt in 0..=config.max_retries {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms as f64 * config.backoff_multiplier) as u64;
            backoff_ms = backoff_ms.min(config.max_backoff_ms);
        }

        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt < config.max_retries {
                    warn!(
                        attempt = attempt + 1,
                        max_retries = config.max_retries,
                        error = %e,
                        "Retryable error, retrying"
                    );
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    if let Some(err) = last_error {
        Err(err)
    } else {
        Err(VectorClientError::InternalError(
            "Max retries exceeded".to_string(),
        ))
    }
}
