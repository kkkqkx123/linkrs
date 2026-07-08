//! Busy Waiting Processor Module
//!
//! Provide concurrency control mechanism in multi-threaded environments, support timeout and exponential backoff waiting strategy

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Busy Waiting Processor
///
/// Used to handle resource conflicts when multiple threads access the database at the same time
/// Supports exponential backoff algorithm to avoid busy waiting consuming too much CPU.
#[derive(Debug)]
pub struct BusyHandler {
    /// Timeout time (milliseconds), 0 means no wait
    timeout_ms: u32,
    /// Current Retries
    retry_count: AtomicU32,
    /// Starting time
    start_time: Instant,
}

impl BusyHandler {
    /// Creating a new Busy Waiting Processor
    ///
    /// # Parameters
    /// - `timeout_ms` - timeout in milliseconds, 0 means no wait
    ///
    /// # Examples
    ///
    /// ```rust
    /// use graphdb::api::embedded::BusyHandler;
    ///
    /// let handler = BusyHandler::new(5000); // 5 second timeout
    /// ```
    pub fn new(timeout_ms: u32) -> Self {
        Self {
            timeout_ms,
            retry_count: AtomicU32::new(0),
            start_time: Instant::now(),
        }
    }

    /// Busy status is handled
    ///
    /// Returns true to continue waiting, false to abort (timeout)
    ///
    /// # Description
    ///
    /// Calculate the waiting time using the exponential backoff algorithm:
    /// - 0th: 1ms
    /// - 1st: 2ms
    /// - 2nd: 4ms
    /// - ...
    /// - 100ms max.
    pub fn handle_busy(&self) -> bool {
        // no-wait mode
        if self.timeout_ms == 0 {
            return false;
        }

        let count = self.retry_count.fetch_add(1, Ordering::SeqCst);

        // Check for timeouts
        let elapsed = self.start_time.elapsed().as_millis() as u64;
        if elapsed >= self.timeout_ms as u64 {
            return false;
        }

        // Calculation of waiting time (exponential retreat)
        let wait_ms = Self::calculate_wait_time(count);

        // Ensure that the remaining timeout is not exceeded
        let remaining = self.timeout_ms as u64 - elapsed;
        let actual_wait = std::cmp::min(wait_ms, remaining);

        std::thread::sleep(Duration::from_millis(actual_wait));
        true
    }

    /// Check if timeout has expired
    pub fn is_timeout(&self) -> bool {
        if self.timeout_ms == 0 {
            return true;
        }
        self.start_time.elapsed().as_millis() as u64 >= self.timeout_ms as u64
    }

    /// Get current retry count
    pub fn retry_count(&self) -> u32 {
        self.retry_count.load(Ordering::SeqCst)
    }

    /// Get the waited time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Reset processor state
    pub fn reset(&self) {
        self.retry_count.store(0, Ordering::SeqCst);
    }

    /// Calculation of waiting time (exponential retreat)
    ///
    /// Formula: min(2^retry_count, 100) milliseconds
    fn calculate_wait_time(retry_count: u32) -> u64 {
        let base = 1u64;
        let max_wait = 100u64; // Maximum 100ms

        // Shift overflow prevention
        if retry_count >= 63 {
            return max_wait;
        }

        std::cmp::min(base << retry_count, max_wait)
    }
}

impl Clone for BusyHandler {
    fn clone(&self) -> Self {
        Self {
            timeout_ms: self.timeout_ms,
            retry_count: AtomicU32::new(0),
            start_time: Instant::now(),
        }
    }
}

/// Busy waiting for results
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyResult {
    /// Successful access to resources
    Success,
    /// overtime pay
    Timeout,
    /// Giving up (not waiting)
    Abort,
}

/// Busy Waiting Configuration
#[derive(Debug, Clone, Copy)]
pub struct BusyConfig {
    /// Timeout time (milliseconds)
    pub timeout_ms: u32,
    /// Maximum number of retries (0 means unlimited)
    pub max_retries: u32,
}

impl BusyConfig {
    /// Creating a New Busy Waiting Configuration
    pub fn new(timeout_ms: u32) -> Self {
        Self {
            timeout_ms,
            max_retries: 0, // limitless
        }
    }

    /// Setting the maximum number of retries
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Disable busy wait (immediate failure)
    pub fn no_wait() -> Self {
        Self {
            timeout_ms: 0,
            max_retries: 0,
        }
    }
}

impl Default for BusyConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 5000, // Default 5 seconds
            max_retries: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_busy_handler_no_wait() {
        let handler = BusyHandler::new(0);
        assert!(!handler.handle_busy());
        assert!(handler.is_timeout());
    }

    #[test]
    fn test_busy_handler_wait() {
        let handler = BusyHandler::new(100); // 100ms timeout
        assert!(handler.handle_busy()); // should return true the first time
        assert!(!handler.is_timeout());
        assert_eq!(handler.retry_count(), 1);
    }

    #[test]
    fn test_busy_handler_timeout() {
        let handler = BusyHandler::new(1); // 1ms timeout
        std::thread::sleep(Duration::from_millis(2));
        assert!(!handler.handle_busy()); // It should time out.
        assert!(handler.is_timeout());
    }

    #[test]
    fn test_calculate_wait_time() {
        assert_eq!(BusyHandler::calculate_wait_time(0), 1);
        assert_eq!(BusyHandler::calculate_wait_time(1), 2);
        assert_eq!(BusyHandler::calculate_wait_time(2), 4);
        assert_eq!(BusyHandler::calculate_wait_time(6), 64);
        assert_eq!(BusyHandler::calculate_wait_time(7), 100); // hit one's maximum value
        assert_eq!(BusyHandler::calculate_wait_time(10), 100); // Maintaining the maximum value
    }

    #[test]
    fn test_busy_config() {
        let config = BusyConfig::default();
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.max_retries, 0);

        let config = BusyConfig::new(1000).with_max_retries(10);
        assert_eq!(config.timeout_ms, 1000);
        assert_eq!(config.max_retries, 10);

        let config = BusyConfig::no_wait();
        assert_eq!(config.timeout_ms, 0);
    }
}
