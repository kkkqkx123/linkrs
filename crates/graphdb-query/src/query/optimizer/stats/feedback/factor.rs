//! Generic feedback-driven correction factor
//!
//! Corrects an estimated value (selectivity, row count, ...) using an
//! exponentially weighted moving average (EWMA) of observed
//! estimated-vs-actual ratios.  The correction factor starts at 1.0 and is
//! clamped to a configurable range to avoid over-correction.

/// EWMA-driven correction factor for a single estimated value.
#[derive(Debug, Clone)]
pub struct FeedbackDrivenFactor {
    /// Original (un-corrected) estimate.
    estimated: f64,
    /// EWMA of the correction factor (actual / estimated).
    correction_factor: f64,
    /// Number of feedbacks received.
    feedback_count: u64,
    /// EWMA smoothing factor.
    alpha: f64,
    /// Minimum correction factor.
    min_correction: f64,
    /// Maximum correction factor.
    max_correction: f64,
    /// Cumulative estimation error (used for error statistics).
    cumulative_estimation_error: f64,
    /// Sum of the squared errors (used to calculate the standard deviation).
    error_sum_squares: f64,
}

impl FeedbackDrivenFactor {
    /// Create a factor for `estimated` with default parameters.
    pub fn new(estimated: f64) -> Self {
        Self {
            estimated,
            correction_factor: 1.0,
            feedback_count: 0,
            alpha: 0.3,
            min_correction: 0.1,
            max_correction: 10.0,
            cumulative_estimation_error: 0.0,
            error_sum_squares: 0.0,
        }
    }

    /// Create a factor with custom parameters.
    pub fn with_params(estimated: f64, alpha: f64, min_correction: f64, max_correction: f64) -> Self {
        Self {
            estimated,
            correction_factor: 1.0,
            feedback_count: 0,
            alpha,
            min_correction,
            max_correction,
            cumulative_estimation_error: 0.0,
            error_sum_squares: 0.0,
        }
    }

    /// The uncorrected estimate.
    pub fn estimated(&self) -> f64 {
        self.estimated
    }

    /// The corrected value: `estimated * correction_factor`.
    pub fn corrected(&self) -> f64 {
        self.estimated * self.correction_factor
    }

    /// The current correction factor.
    pub fn correction_factor(&self) -> f64 {
        self.correction_factor
    }

    /// Number of feedbacks received.
    pub fn feedback_count(&self) -> u64 {
        self.feedback_count
    }

    /// Confidence in the correction, based on the feedback count.
    ///
    /// Sigmoid in the feedback count; close to 0.9 at 100 feedbacks.
    pub fn estimation_confidence(&self) -> f64 {
        let x = self.feedback_count as f64 * 0.1;
        1.0 / (1.0 + (-x).exp())
    }

    /// Average estimation error of the correction factor.
    pub fn avg_estimation_error(&self) -> f64 {
        if self.feedback_count == 0 {
            return 1.0;
        }
        self.cumulative_estimation_error / self.feedback_count as f64
    }

    /// Standard deviation of the correction-factor errors.
    pub fn error_std_dev(&self) -> f64 {
        if self.feedback_count < 2 {
            return 0.0;
        }
        let n = self.feedback_count as f64;
        let mean = self.cumulative_estimation_error / n;
        let variance = (self.error_sum_squares / n) - (mean * mean);
        variance.max(0.0).sqrt()
    }

    /// Update the correction factor from an observed estimated-vs-actual
    /// ratio (`actual / estimated`), smoothed with EWMA.
    pub fn update_with_ratio(&mut self, ratio: f64) {
        if self.estimated <= 0.0 || !ratio.is_finite() {
            return;
        }
        let error = (ratio - self.correction_factor).abs();
        self.cumulative_estimation_error += error;
        self.error_sum_squares += error * error;
        self.correction_factor = (1.0 - self.alpha) * self.correction_factor + self.alpha * ratio;
        self.correction_factor = self
            .correction_factor
            .clamp(self.min_correction, self.max_correction);
        self.feedback_count += 1;
    }

    /// Reset the correction.
    pub fn reset(&mut self) {
        self.correction_factor = 1.0;
        self.feedback_count = 0;
        self.cumulative_estimation_error = 0.0;
        self.error_sum_squares = 0.0;
    }

    /// Set the EWMA smoothing factor.
    pub fn set_alpha(&mut self, alpha: f64) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    /// Set the correction range.
    pub fn set_correction_range(&mut self, min: f64, max: f64) {
        self.min_correction = min.max(0.01);
        self.max_correction = max.max(self.min_correction);
    }
}

impl Default for FeedbackDrivenFactor {
    fn default() -> Self {
        Self::new(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correction_converges_to_actual() {
        let mut factor = FeedbackDrivenFactor::new(100.0);
        for _ in 0..200 {
            factor.update_with_ratio(2.0); // actual is twice the estimate
        }
        assert!(factor.correction_factor() > 1.8);
        assert!((factor.corrected() - 200.0).abs() < 25.0);
    }

    #[test]
    fn test_correction_is_clamped() {
        let mut factor = FeedbackDrivenFactor::with_params(100.0, 0.3, 0.1, 10.0);
        for _ in 0..100 {
            factor.update_with_ratio(100.0);
        }
        assert!(factor.correction_factor() <= 10.0);
    }

    #[test]
    fn test_confidence_grows_with_feedback() {
        let mut factor = FeedbackDrivenFactor::new(1.0);
        assert!(factor.estimation_confidence() < 0.55);
        for i in 0..100 {
            factor.update_with_ratio(1.0 + i as f64 * 0.001);
        }
        assert!(factor.estimation_confidence() > 0.9);
    }

    #[test]
    fn test_ignores_invalid_ratios() {
        let mut factor = FeedbackDrivenFactor::new(10.0);
        factor.update_with_ratio(f64::NAN);
        factor.update_with_ratio(f64::INFINITY);
        assert_eq!(factor.feedback_count(), 0);
    }
}
