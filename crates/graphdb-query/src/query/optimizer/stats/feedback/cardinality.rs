//! Cardinality feedback manager
//!
//! Corrects per-operator output row estimates with EWMA correction factors
//! keyed by normalized operator shape keys (`"{space}:{Type}:{discriminator}"`).
//!
//! This is the row-count counterpart of the predicate-level
//! [`SelectivityFeedbackManager`](super::selectivity::SelectivityFeedbackManager):
//! predicates are corrected per normalized condition, whole-operator
//! cardinalities per normalized operator shape.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::factor::FeedbackDrivenFactor;

/// Row-count correction manager shared across query executions.
#[derive(Debug, Default)]
pub struct CardinalityFeedbackManager {
    /// Mapping from operator shape key to correction factor.
    feedbacks: RwLock<HashMap<String, FeedbackDrivenFactor>>,
    /// Default EWMA smoothing factor.
    default_alpha: f64,
    /// Default minimum correction factor.
    default_min_correction: f64,
    /// Default maximum correction factor.
    default_max_correction: f64,
}

impl CardinalityFeedbackManager {
    /// Create a new manager.
    pub fn new() -> Self {
        Self::with_params(0.3, 0.1, 10.0)
    }

    /// Create a manager with custom default parameters.
    pub fn with_params(alpha: f64, min_correction: f64, max_correction: f64) -> Self {
        Self {
            feedbacks: RwLock::new(HashMap::new()),
            default_alpha: alpha,
            default_min_correction: min_correction,
            default_max_correction: max_correction,
        }
    }

    /// Register an estimate for a shape key.
    ///
    /// Registers only when the key is not present yet, so repeated estimation
    /// of the same operator shape does not reset learned corrections.
    pub fn register_key(&self, key: String, estimated_rows: f64) {
        let feedback = FeedbackDrivenFactor::with_params(
            estimated_rows,
            self.default_alpha,
            self.default_min_correction,
            self.default_max_correction,
        );
        self.feedbacks.write().entry(key).or_insert(feedback);
    }

    /// The corrected row estimate for a shape key, if registered.
    pub fn corrected_rows(&self, key: &str) -> Option<f64> {
        self.feedbacks
            .read()
            .get(key)
            .map(|f| f.corrected())
            .filter(|v| *v > 0.0)
    }

    /// The correction factor for a shape key, if registered.
    pub fn correction_factor(&self, key: &str) -> Option<f64> {
        self.feedbacks.read().get(key).map(|f| f.correction_factor())
    }

    /// Update feedback from an estimated-vs-actual row ratio.
    ///
    /// The ratio is `actual_rows / estimated_rows`; returns `false` when the
    /// key has not been registered.
    pub fn update_feedback_ratio(&self, key: &str, ratio: f64) -> bool {
        let mut feedbacks = self.feedbacks.write();
        if let Some(feedback) = feedbacks.get_mut(key) {
            feedback.update_with_ratio(ratio);
            true
        } else {
            false
        }
    }

    /// Remove all feedback whose key starts with `space_prefix` (e.g.
    /// `"myspace:"`), returning the number of removed entries.
    pub fn remove_feedback_by_space(&self, space_prefix: &str) -> usize {
        let mut feedbacks = self.feedbacks.write();
        let mut removed = 0;
        feedbacks.retain(|key, _| {
            let keep = !key.starts_with(space_prefix);
            if !keep {
                removed += 1;
            }
            keep
        });
        removed
    }

    /// Remove feedback for a specific key.
    pub fn remove_feedback(&self, key: &str) -> Option<FeedbackDrivenFactor> {
        self.feedbacks.write().remove(key)
    }

    /// All registered keys.
    pub fn get_all_keys(&self) -> Vec<String> {
        self.feedbacks.read().keys().cloned().collect()
    }

    /// Number of registered keys.
    pub fn feedback_count(&self) -> usize {
        self.feedbacks.read().len()
    }

    /// Clear all feedback.
    pub fn clear_all(&self) {
        self.feedbacks.write().clear();
    }

    /// The correction factor for a key, if any.
    pub fn get_feedback(&self, key: &str) -> Option<FeedbackDrivenFactor> {
        self.feedbacks.read().get(key).cloned()
    }
}

impl Clone for CardinalityFeedbackManager {
    fn clone(&self) -> Self {
        Self {
            feedbacks: RwLock::new(self.feedbacks.read().clone()),
            default_alpha: self.default_alpha,
            default_min_correction: self.default_min_correction,
            default_max_correction: self.default_max_correction,
        }
    }
}

/// Thread-safe handle to a shared [`CardinalityFeedbackManager`].
pub type SharedCardinalityFeedback = Arc<CardinalityFeedbackManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_correct() {
        let manager = CardinalityFeedbackManager::new();
        manager.register_key("s:ScanVertices:Person".to_string(), 100.0);
        assert_eq!(
            manager.corrected_rows("s:ScanVertices:Person"),
            Some(100.0)
        );
        for _ in 0..50 {
            manager.update_feedback_ratio("s:ScanVertices:Person", 3.0);
        }
        let corrected = manager.corrected_rows("s:ScanVertices:Person").unwrap();
        assert!(corrected > 200.0 && corrected <= 1000.0);
    }

    #[test]
    fn test_register_is_idempotent() {
        let manager = CardinalityFeedbackManager::new();
        manager.register_key("k".to_string(), 10.0);
        manager.register_key("k".to_string(), 1000.0);
        assert_eq!(manager.corrected_rows("k"), Some(10.0));
    }

    #[test]
    fn test_unknown_key_updates_return_false() {
        let manager = CardinalityFeedbackManager::new();
        assert!(!manager.update_feedback_ratio("missing", 2.0));
    }

    #[test]
    fn test_remove_by_space() {
        let manager = CardinalityFeedbackManager::new();
        manager.register_key("a:ScanVertices".to_string(), 10.0);
        manager.register_key("b:ScanVertices".to_string(), 10.0);
        assert_eq!(manager.remove_feedback_by_space("a:"), 1);
        assert_eq!(manager.feedback_count(), 1);
    }
}
