//! Automatic feedback triggering module
//!
//! Refer to PostgreSQL’s ANALYZE automatic trigger mechanism.
//! Configure when statistical information should be automatically updated and when the selection criteria should be re-evaluated.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Automatic feedback triggering configuration
///
/// Configure when statistical updates should be triggered automatically, including the minimum number of samples, the error threshold, and the cooling period.
///
/// # Example
/// ```
/// use graphdb::query::optimizer::stats::feedback::trigger::AutoFeedbackConfig;
///
/// let config = AutoFeedbackConfig::new();
/// assert!(config.enabled);
/// assert_eq!(config.min_samples_for_update, 10);
/// ```
#[derive(Debug, Clone)]
pub struct AutoFeedbackConfig {
    /// The minimum number of feedback samples triggers a re-evaluation.
    pub min_samples_for_update: usize,
    /// The error threshold triggers an emergency update (the update is performed immediately when the error exceeds this value).
    pub error_threshold: f64,
    /// Update the cooling time (to avoid frequent updates).
    pub update_cooldown_ms: u64,
    /// Maximum number of feedback records in history
    pub max_feedback_history: usize,
    /// Should automatic updates be enabled?
    pub enabled: bool,
}

impl AutoFeedbackConfig {
    /// Create the default configuration.
    pub fn new() -> Self {
        Self {
            min_samples_for_update: 10,
            error_threshold: 0.5,
            update_cooldown_ms: 60000, // 1 minute
            max_feedback_history: 100,
            enabled: true,
        }
    }

    /// Create using custom parameters
    pub fn with_params(
        min_samples: usize,
        error_threshold: f64,
        cooldown_ms: u64,
        max_history: usize,
    ) -> Self {
        Self {
            min_samples_for_update: min_samples,
            error_threshold: error_threshold.clamp(0.1, 1.0),
            update_cooldown_ms: cooldown_ms,
            max_feedback_history: max_history,
            enabled: true,
        }
    }

    /// Check whether an update should be triggered.
    ///
    /// # Parameters
    /// `feedback_count`: The current number of feedback samples.
    /// `last_update_ms`: Time of the last update (in milliseconds as a timestamp)
    /// `current_error`: The current estimated error
    ///
    /// # Return
    /// `true`: The update should be triggered.
    /// `false`: No update is required.
    pub fn should_trigger_update(
        &self,
        feedback_count: usize,
        last_update_ms: u64,
        current_error: f64,
    ) -> bool {
        if !self.enabled {
            return false;
        }

        // Check the error threshold (urgent update).
        if current_error > self.error_threshold {
            return true;
        }

        // Check the minimum sample size.
        if feedback_count < self.min_samples_for_update {
            return false;
        }

        // Check the cooling time.
        let current_time = Instant::now().elapsed().as_millis() as u64;
        if current_time.saturating_sub(last_update_ms) < self.update_cooldown_ms {
            return false;
        }

        true
    }

    /// Enable automatic updates
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable automatic updates.
    pub fn disable(&mut self) {
        self.enabled = false;
    }
}

impl Default for AutoFeedbackConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Automatic feedback trigger
///
/// Automatically determines whether to trigger a statistical update based on the configuration.
/// Use atomic operations to ensure thread safety.
///
/// # Examples
/// ```
/// use graphdb::query::optimizer::stats::feedback::trigger::{AutoFeedbackTrigger, AutoFeedbackConfig};
///
/// let config = AutoFeedbackConfig::with_params(5, 0.5, 1000, 50);
/// let trigger = AutoFeedbackTrigger::new(config);
///
// Record feedback
/// for _ in 0..5 {
///     trigger.record_feedback();
/// }
///
// Check whether an action should be triggered (if the error exceeds the threshold)
/// assert!(trigger.should_trigger(0.6));
/// ```
#[derive(Debug)]
pub struct AutoFeedbackTrigger {
    /// Configuration
    config: AutoFeedbackConfig,
    /// Last update time
    last_update_time_ms: AtomicU64,
    /// Current feedback count
    feedback_count: AtomicU64,
}

impl AutoFeedbackTrigger {
    /// Create a new trigger.
    pub fn new(config: AutoFeedbackConfig) -> Self {
        Self {
            config,
            last_update_time_ms: AtomicU64::new(0),
            feedback_count: AtomicU64::new(0),
        }
    }

    /// Record feedback
    pub fn record_feedback(&self) {
        self.feedback_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Check if an update should be triggered
    pub fn should_trigger(&self, current_error: f64) -> bool {
        let count = self.feedback_count.load(Ordering::Relaxed) as usize;
        let last_update = self.last_update_time_ms.load(Ordering::Relaxed);
        self.config
            .should_trigger_update(count, last_update, current_error)
    }

    /// The marking update has been completed.
    pub fn mark_updated(&self) {
        let current_time = Instant::now().elapsed().as_millis() as u64;
        self.last_update_time_ms
            .store(current_time, Ordering::Relaxed);
        self.feedback_count.store(0, Ordering::Relaxed);
    }

    /// Obtain the current count of feedback messages.
    pub fn get_feedback_count(&self) -> u64 {
        self.feedback_count.load(Ordering::Relaxed)
    }

    /// Update the configuration.
    pub fn update_config(&mut self, config: AutoFeedbackConfig) {
        self.config = config;
    }

    /// Obtain the configuration.
    pub fn config(&self) -> &AutoFeedbackConfig {
        &self.config
    }
}

impl Default for AutoFeedbackTrigger {
    fn default() -> Self {
        Self::new(AutoFeedbackConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_feedback_config() {
        let config = AutoFeedbackConfig::new();
        assert!(config.enabled);
        assert_eq!(config.min_samples_for_update, 10);

        // Test trigger logic
        assert!(!config.should_trigger_update(5, 0, 0.1)); // Insufficient sample size
        assert!(config.should_trigger_update(5, 0, 0.6)); // The error exceeds the threshold (urgent update).

        // Test the normal triggering logic: If the program has been running for longer than the cooling period…
        let current_time = Instant::now().elapsed().as_millis() as u64;
        if current_time > config.update_cooldown_ms {
            assert!(config.should_trigger_update(15, 0, 0.1)); // The sample is sufficient, and the cooling time has already passed.
        }
    }

    #[test]
    fn test_auto_feedback_trigger() {
        let config = AutoFeedbackConfig::with_params(5, 0.5, 1000, 50);
        let trigger = AutoFeedbackTrigger::new(config);

        assert!(!trigger.should_trigger(0.1)); // No feedback.

        // Record feedback
        for _ in 0..5 {
            trigger.record_feedback();
        }

        // An error exceeding the threshold should trigger a response.
        assert!(trigger.should_trigger(0.6));

        trigger.mark_updated();
        assert_eq!(trigger.get_feedback_count(), 0);
    }

    #[test]
    fn test_config_enable_disable() {
        let mut config = AutoFeedbackConfig::new();
        assert!(config.enabled);

        config.disable();
        assert!(!config.enabled);
        assert!(!config.should_trigger_update(100, 0, 1.0));

        config.enable();
        assert!(config.enabled);
    }
}
