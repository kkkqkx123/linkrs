//! Selective Correction Module
//!
//! Provide the functionality to dynamically adjust selective estimates based on historical performance feedback.
//! 使用指数加权移动平均(EWMA)算法校正选择性估计。

use parking_lot::RwLock;
use std::collections::HashMap;

/// Feedback-driven selective correction
///
/// Dynamically adjust the selective estimates based on historical feedback from executions.
/// 使用指数加权移动平均(EWMA)算法。
///
/// # Example
/// ```
/// use graphdb::query::optimizer::stats::feedback::selectivity::FeedbackDrivenSelectivity;
///
/// let mut feedback = FeedbackDrivenSelectivity::new(0.1);
/// assert_eq!(feedback.estimated_selectivity(), 0.1);
///
// Update the feedback
/// feedback.update_with_feedback(0.15);
/// feedback.update_with_feedback(0.12);
///
// Obtain the corrected selective information
/// let corrected = feedback.corrected_selectivity();
/// assert!(corrected > 0.0 && corrected <= 0.99);
/// ```
#[derive(Debug, Clone)]
pub struct FeedbackDrivenSelectivity {
    /// Original estimate of selectivity
    estimated_selectivity: f64,
    /// Historical Selectivity (Sliding Window Average)
    actual_selectivity_ewma: f64,
    /// Correction factor
    correction_factor: f64,
    /// Number of feedbacks
    feedback_count: u64,
    /// EWMA smoothing factor
    alpha: f64,
    /// Minimum correction factor
    min_correction: f64,
    /// Maximum correction factor
    max_correction: f64,
    /// Selective upper limit (default 0.99)
    selectivity_cap: f64,
    /// Cumulative estimated error (used for calculating error statistics)
    cumulative_estimation_error: f64,
    /// Sum of the squared errors (used to calculate the standard deviation)
    error_sum_squares: f64,
}

impl FeedbackDrivenSelectivity {
    /// Create a new feedback-driven, selective estimation method
    pub fn new(estimated_selectivity: f64) -> Self {
        Self {
            estimated_selectivity,
            actual_selectivity_ewma: estimated_selectivity,
            correction_factor: 1.0,
            feedback_count: 0,
            alpha: 0.3,
            min_correction: 0.1,
            max_correction: 10.0,
            selectivity_cap: 0.99,
            cumulative_estimation_error: 0.0,
            error_sum_squares: 0.0,
        }
    }

    /// Create using custom parameters
    pub fn with_params(
        estimated_selectivity: f64,
        alpha: f64,
        min_correction: f64,
        max_correction: f64,
    ) -> Self {
        Self {
            estimated_selectivity,
            actual_selectivity_ewma: estimated_selectivity,
            correction_factor: 1.0,
            feedback_count: 0,
            alpha,
            min_correction,
            max_correction,
            selectivity_cap: 0.99,
            cumulative_estimation_error: 0.0,
            error_sum_squares: 0.0,
        }
    }

    /// Obtain the original estimate for selectivity.
    pub fn estimated_selectivity(&self) -> f64 {
        self.estimated_selectivity
    }

    /// Obtain the corrected version of the selective content.
    pub fn corrected_selectivity(&self) -> f64 {
        (self.estimated_selectivity * self.correction_factor).clamp(
            self.min_correction * self.estimated_selectivity,
            self.selectivity_cap,
        )
    }

    /// Obtain the correction factor
    pub fn correction_factor(&self) -> f64 {
        self.correction_factor
    }

    /// Number of times feedback was obtained
    pub fn feedback_count(&self) -> u64 {
        self.feedback_count
    }

    /// Obtaining the confidence level for the selected estimate
    ///
    /// The confidence level is calculated based on the number of feedbacks received; the more feedback, the higher the confidence level.
    /// Return value range: 0.0 - 1.0
    pub fn estimation_confidence(&self) -> f64 {
        // Calculate the confidence using the sigmoid function.
        // The confidence level is close to 0.9 when the number of feedbacks reaches 100
        let x = self.feedback_count as f64 * 0.1;
        1.0 / (1.0 + (-x).exp())
    }

    /// Obtain the average estimated error.
    pub fn avg_estimation_error(&self) -> f64 {
        if self.feedback_count == 0 {
            return 1.0;
        }
        self.cumulative_estimation_error / self.feedback_count as f64
    }

    /// Obtain the standard deviation of the estimated error
    pub fn error_std_dev(&self) -> f64 {
        if self.feedback_count < 2 {
            return 0.0;
        }
        let n = self.feedback_count as f64;
        let mean = self.cumulative_estimation_error / n;
        let variance = (self.error_sum_squares / n) - (mean * mean);
        variance.max(0.0).sqrt()
    }

    /// Update the correction factor (based on the new feedback).
    ///
    /// 使用指数加权移动平均(EWMA)算法。
    pub fn update_with_feedback(&mut self, actual_selectivity: f64) {
        if self.estimated_selectivity <= 0.0 {
            return;
        }

        let ratio = actual_selectivity / self.estimated_selectivity;

        // Calculate the current estimated error.
        let estimated = self.corrected_selectivity();
        let error = (actual_selectivity - estimated).abs();
        self.cumulative_estimation_error += error;
        self.error_sum_squares += error * error;

        // Use the EWMA to update the correction factor.
        self.correction_factor = (1.0 - self.alpha) * self.correction_factor + self.alpha * ratio;

        // Limit the range of the correction factors to avoid excessive correction.
        self.correction_factor = self
            .correction_factor
            .clamp(self.min_correction, self.max_correction);

        // Updated version of the practical selective EWMA (Exponential Moving Average)
        self.actual_selectivity_ewma =
            (1.0 - self.alpha) * self.actual_selectivity_ewma + self.alpha * actual_selectivity;

        self.feedback_count += 1;
    }

    /// Batch update of feedback
    pub fn update_with_batch(&mut self, actual_selectivities: &[f64]) {
        for &selectivity in actual_selectivities {
            self.update_with_feedback(selectivity);
        }
    }

    /// Reset the correction factor
    pub fn reset_correction(&mut self) {
        self.correction_factor = 1.0;
        self.actual_selectivity_ewma = self.estimated_selectivity;
        self.feedback_count = 0;
        self.cumulative_estimation_error = 0.0;
        self.error_sum_squares = 0.0;
    }

    /// Setting the EWMA smoothing factor
    pub fn set_alpha(&mut self, alpha: f64) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    /// Set the correction range
    pub fn set_correction_range(&mut self, min: f64, max: f64) {
        self.min_correction = min.max(0.01);
        self.max_correction = max.max(self.min_correction);
    }

    /// Setting a selective upper limit
    pub fn set_selectivity_cap(&mut self, cap: f64) {
        self.selectivity_cap = cap.clamp(0.5, 1.0);
    }
}

impl Default for FeedbackDrivenSelectivity {
    fn default() -> Self {
        Self::new(0.1)
    }
}

/// Selective Feedback Manager
///
/// Managing selective feedback based on multiple conditions.
///
/// # Examples
/// ```
/// use graphdb::query::optimizer::stats::feedback::selectivity::SelectivityFeedbackManager;
///
/// let manager = SelectivityFeedbackManager::new();
/// manager.register_condition("age > 25".to_string(), 0.3);
///
/// let corrected = manager.get_corrected_selectivity("age > 25");
/// assert!(corrected.is_some());
/// ```
#[derive(Debug)]
pub struct SelectivityFeedbackManager {
    /// Mapping from condition keys to selective correction
    feedbacks: RwLock<HashMap<String, FeedbackDrivenSelectivity>>,
    /// Default EWMA smoothing factor
    default_alpha: f64,
    /// Default minimum correction factor
    default_min_correction: f64,
    /// Default maximum correction factor
    default_max_correction: f64,
}

impl SelectivityFeedbackManager {
    /// Create a new feedback manager.
    pub fn new() -> Self {
        Self {
            feedbacks: RwLock::new(HashMap::new()),
            default_alpha: 0.3,
            default_min_correction: 0.1,
            default_max_correction: 10.0,
        }
    }

    /// Created using custom parameters
    pub fn with_params(alpha: f64, min_correction: f64, max_correction: f64) -> Self {
        Self {
            feedbacks: RwLock::new(HashMap::new()),
            default_alpha: alpha,
            default_min_correction: min_correction,
            default_max_correction: max_correction,
        }
    }

    /// Selective estimation of registration requirements
    pub fn register_condition(&self, key: String, estimated_selectivity: f64) {
        let feedback = FeedbackDrivenSelectivity::with_params(
            estimated_selectivity,
            self.default_alpha,
            self.default_min_correction,
            self.default_max_correction,
        );
        self.feedbacks.write().insert(key, feedback);
    }

    /// Getting corrected selectivity
    pub fn get_corrected_selectivity(&self, key: &str) -> Option<f64> {
        self.feedbacks
            .read()
            .get(key)
            .map(|f| f.corrected_selectivity())
    }

    /// Update feedback
    pub fn update_feedback(&self, key: &str, actual_selectivity: f64) -> bool {
        let mut feedbacks = self.feedbacks.write();
        if let Some(feedback) = feedbacks.get_mut(key) {
            feedback.update_with_feedback(actual_selectivity);
            true
        } else {
            false
        }
    }

    /// Batch Update Feedback
    pub fn update_feedback_batch(&self, updates: &[(String, f64)]) {
        let mut feedbacks = self.feedbacks.write();
        for (key, actual_selectivity) in updates {
            if let Some(feedback) = feedbacks.get_mut(key) {
                feedback.update_with_feedback(*actual_selectivity);
            }
        }
    }

    /// Obtain feedback information
    pub fn get_feedback(&self, key: &str) -> Option<FeedbackDrivenSelectivity> {
        self.feedbacks.read().get(key).cloned()
    }

    /// Obtain all feedback keys.
    pub fn get_all_keys(&self) -> Vec<String> {
        self.feedbacks.read().keys().cloned().collect()
    }

    /// Clear all feedback.
    pub fn clear_all(&self) {
        self.feedbacks.write().clear();
    }

    /// Remove feedback that meets specific criteria.
    pub fn remove_feedback(&self, key: &str) -> Option<FeedbackDrivenSelectivity> {
        self.feedbacks.write().remove(key)
    }

    /// Number of feedback requests received
    pub fn feedback_count(&self) -> usize {
        self.feedbacks.read().len()
    }

    /// Set default parameters
    pub fn set_default_params(&mut self, alpha: f64, min_correction: f64, max_correction: f64) {
        self.default_alpha = alpha.clamp(0.0, 1.0);
        self.default_min_correction = min_correction.max(0.01);
        self.default_max_correction = max_correction.max(self.default_min_correction);
    }
}

impl Default for SelectivityFeedbackManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SelectivityFeedbackManager {
    fn clone(&self) -> Self {
        Self {
            feedbacks: RwLock::new(self.feedbacks.read().clone()),
            default_alpha: self.default_alpha,
            default_min_correction: self.default_min_correction,
            default_max_correction: self.default_max_correction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feedback_driven_selectivity() {
        let mut feedback = FeedbackDrivenSelectivity::new(0.1);
        assert_eq!(feedback.estimated_selectivity(), 0.1);

        // Updated feedback
        feedback.update_with_feedback(0.15);
        feedback.update_with_feedback(0.12);

        // The corrected selectivity should be within a reasonable range.
        let corrected = feedback.corrected_selectivity();
        assert!(corrected > 0.0 && corrected <= 0.99);

        // The number of feedback instances should be 2.
        assert_eq!(feedback.feedback_count(), 2);
    }

    #[test]
    fn test_feedback_correction_range() {
        let mut feedback = FeedbackDrivenSelectivity::new(0.5);

        // Numerous updates; testing of the correction factor limitations
        for _ in 0..100 {
            feedback.update_with_feedback(0.01); // Far lower than the estimated values
        }

        // The correction factor should be limited to its minimum value.
        assert!(feedback.correction_factor() >= 0.1);
    }

    #[test]
    fn test_estimation_confidence() {
        let mut feedback = FeedbackDrivenSelectivity::new(0.1);
        // 初始置信度应该是0.5（sigmoid(0) = 0.5）
        let initial_confidence = feedback.estimation_confidence();
        assert!(initial_confidence < 0.55 && initial_confidence > 0.45);

        // Add multiple feedback entries.
        for i in 0..100 {
            feedback.update_with_feedback(0.1 + (i as f64 * 0.001));
        }

        assert!(feedback.estimation_confidence() > 0.9); // High confidence level after receiving the feedback
    }

    #[test]
    fn test_avg_estimation_error() {
        let mut feedback = FeedbackDrivenSelectivity::new(0.5);

        // 1.0% error without feedback
        assert_eq!(feedback.avg_estimation_error(), 1.0);

        // Add feedback
        feedback.update_with_feedback(0.5); // No errors.
        feedback.update_with_feedback(0.6); // There are errors.

        assert!(feedback.avg_estimation_error() < 1.0);
    }

    #[test]
    fn test_selectivity_feedback_manager() {
        let manager = SelectivityFeedbackManager::new();
        manager.register_condition("age > 25".to_string(), 0.3);
        manager.register_condition("salary > 5000".to_string(), 0.2);

        assert_eq!(manager.feedback_count(), 2);

        // Updated feedback
        manager.update_feedback("age > 25", 0.35);
        manager.update_feedback("salary > 5000", 0.18);

        // Obtain the corrected version of the selective content.
        let corrected_age = manager.get_corrected_selectivity("age > 25");
        assert!(corrected_age.is_some());

        // Trying to obtain a condition that does not exist…
        assert!(manager.get_corrected_selectivity("unknown").is_none());
    }

    #[test]
    fn test_selectivity_cap() {
        let mut feedback = FeedbackDrivenSelectivity::new(0.9);
        feedback.set_selectivity_cap(0.95);

        // Numerous updates have been made in an attempt to increase the level of selectivity beyond the maximum limit.
        for _ in 0..50 {
            feedback.update_with_feedback(1.0);
        }

        // The corrected selectivity should not exceed the upper limit.
        assert!(feedback.corrected_selectivity() <= 0.95);
    }
}
