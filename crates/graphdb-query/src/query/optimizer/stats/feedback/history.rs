//! Query Feedback History Module
//!
//! Management functions for the history of query feedback, including storage, retrieval, and cleanup.

use crate::query::optimizer::stats::feedback::query::QueryExecutionFeedback;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Instant;

/// Query feedback history
///
/// Manage the historical records of query execution feedback, storing them in groups based on the query’s “fingerprint” (unique characteristics that identify each query).
///
/// # Example
/// ```
/// use graphdb::query::optimizer::stats::feedback::history::QueryFeedbackHistory;
/// use graphdb::query::optimizer::stats::feedback::query::QueryExecutionFeedback;
///
/// let history = QueryFeedbackHistory::new(100);
/// let feedback = QueryExecutionFeedback::new("fp_123".to_string());
/// history.add_feedback(feedback);
///
/// let feedbacks = history.get_feedback_for_query("fp_123");
/// assert_eq!(feedbacks.len(), 1);
/// ```
#[derive(Debug)]
pub struct QueryFeedbackHistory {
    /// The mapping from fingerprints to the feedback list
    feedbacks: RwLock<HashMap<String, Vec<QueryExecutionFeedback>>>,
    /// Maximum number of historical records per query
    max_history_per_query: usize,
}

impl QueryFeedbackHistory {
    /// Create a new query feedback history.
    ///
    /// # Parameters
    /// `max_history_per_query`: The maximum number of historical records to be retained for each query.
    pub fn new(max_history_per_query: usize) -> Self {
        Self {
            feedbacks: RwLock::new(HashMap::new()),
            max_history_per_query: max_history_per_query.max(1),
        }
    }

    /// Add feedback on the execution of the query.
    pub fn add_feedback(&self, feedback: QueryExecutionFeedback) {
        let mut feedbacks = self.feedbacks.write();
        let entry = feedbacks
            .entry(feedback.query_fingerprint.clone())
            .or_default();

        entry.push(feedback);

        // Limit the number of historical records.
        if entry.len() > self.max_history_per_query {
            entry.remove(0);
        }
    }

    /// Obtain all feedback for a specific query.
    pub fn get_feedback_for_query(&self, fingerprint: &str) -> Vec<QueryExecutionFeedback> {
        self.feedbacks
            .read()
            .get(fingerprint)
            .cloned()
            .unwrap_or_default()
    }

    /// Obtain the number of feedback responses for a specific query
    pub fn get_feedback_count(&self, fingerprint: &str) -> usize {
        self.feedbacks
            .read()
            .get(fingerprint)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Retrieve all query fingerprints.
    pub fn get_all_fingerprints(&self) -> Vec<String> {
        self.feedbacks.read().keys().cloned().collect()
    }

    /// Clear the history of a specific query
    pub fn clear_query_history(&self, fingerprint: &str) -> bool {
        self.feedbacks.write().remove(fingerprint).is_some()
    }

    /// Clear all history.
    pub fn clear_all(&self) {
        self.feedbacks.write().clear();
    }

    /// Obtain the total number of historical records
    pub fn total_feedback_count(&self) -> usize {
        self.feedbacks.read().values().map(|v| v.len()).sum()
    }

    /// Get the number of queries
    pub fn query_count(&self) -> usize {
        self.feedbacks.read().len()
    }

    /// Retrieve the last N pieces of feedback.
    pub fn get_recent_feedbacks(&self, n: usize) -> Vec<QueryExecutionFeedback> {
        let feedbacks = self.feedbacks.read();
        let mut all_feedbacks: Vec<_> =
            feedbacks.values().flat_map(|v| v.iter().cloned()).collect();

        // Sort by timestamp (latest first)
        all_feedbacks.sort_by(|a, b| {
            b.execution_timestamp
                .elapsed()
                .cmp(&a.execution_timestamp.elapsed())
        });

        all_feedbacks.into_iter().take(n).collect()
    }

    /// Error in estimating the average number of rows in the query results
    pub fn get_avg_row_error(&self, fingerprint: &str) -> Option<f64> {
        let feedbacks = self.feedbacks.read();
        let query_feedbacks = feedbacks.get(fingerprint)?;

        if query_feedbacks.is_empty() {
            return None;
        }

        let total_error: f64 = query_feedbacks
            .iter()
            .map(|f| f.row_estimation_error())
            .sum();
        Some(total_error / query_feedbacks.len() as f64)
    }

    /// The estimated error in the average time taken to obtain the query results
    pub fn get_avg_time_error(&self, fingerprint: &str) -> Option<f64> {
        let feedbacks = self.feedbacks.read();
        let query_feedbacks = feedbacks.get(fingerprint)?;

        if query_feedbacks.is_empty() {
            return None;
        }

        let total_error: f64 = query_feedbacks
            .iter()
            .map(|f| f.time_estimation_error())
            .sum();
        Some(total_error / query_feedbacks.len() as f64)
    }

    /// Clean up outdated historical data (based on time)
    pub fn cleanup_old_feedbacks(&self, max_age: std::time::Duration) {
        let mut feedbacks = self.feedbacks.write();
        let now = Instant::now();

        for query_feedbacks in feedbacks.values_mut() {
            query_feedbacks.retain(|f| now.duration_since(f.execution_timestamp) < max_age);
        }

        // Remove the empty entries.
        feedbacks.retain(|_, v| !v.is_empty());
    }

    /// Set the maximum number of historical records
    pub fn set_max_history(&self, max_history: usize) {
        let max_history = max_history.max(1);
        let mut feedbacks = self.feedbacks.write();

        for query_feedbacks in feedbacks.values_mut() {
            while query_feedbacks.len() > max_history {
                query_feedbacks.remove(0);
            }
        }
    }
}

impl Default for QueryFeedbackHistory {
    fn default() -> Self {
        Self::new(100)
    }
}

impl Clone for QueryFeedbackHistory {
    fn clone(&self) -> Self {
        Self {
            feedbacks: RwLock::new(self.feedbacks.read().clone()),
            max_history_per_query: self.max_history_per_query,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::feedback::query::QueryExecutionFeedback;

    #[test]
    fn test_query_feedback_history() {
        let history = QueryFeedbackHistory::new(10);

        // Add feedback
        let feedback1 = QueryExecutionFeedback::new("fp_123".to_string());
        history.add_feedback(feedback1);

        let feedbacks = history.get_feedback_for_query("fp_123");
        assert_eq!(feedbacks.len(), 1);

        // Add more feedback.
        let feedback2 = QueryExecutionFeedback::new("fp_123".to_string());
        history.add_feedback(feedback2);

        assert_eq!(history.get_feedback_count("fp_123"), 2);
        assert_eq!(history.total_feedback_count(), 2);
        assert_eq!(history.query_count(), 1);
    }

    #[test]
    fn test_history_limit() {
        let history = QueryFeedbackHistory::new(3);

        // Adding 4 feedback entries (exceeding the limit).
        for i in 0..4 {
            let mut feedback = QueryExecutionFeedback::new("fp_123".to_string());
            feedback.actual_rows = i as u64 * 100;
            history.add_feedback(feedback);
        }

        // Only 3 items should be retained.
        assert_eq!(history.get_feedback_count("fp_123"), 3);
    }

    #[test]
    fn test_clear_history() {
        let history = QueryFeedbackHistory::new(10);

        let feedback = QueryExecutionFeedback::new("fp_123".to_string());
        history.add_feedback(feedback);

        assert!(history.clear_query_history("fp_123"));
        assert_eq!(history.get_feedback_count("fp_123"), 0);
        assert!(!history.clear_query_history("nonexistent"));
    }

    #[test]
    fn test_multiple_queries() {
        let history = QueryFeedbackHistory::new(10);

        history.add_feedback(QueryExecutionFeedback::new("fp_1".to_string()));
        history.add_feedback(QueryExecutionFeedback::new("fp_2".to_string()));
        history.add_feedback(QueryExecutionFeedback::new("fp_1".to_string()));

        assert_eq!(history.query_count(), 2);
        assert_eq!(history.total_feedback_count(), 3);

        let fingerprints = history.get_all_fingerprints();
        assert_eq!(fingerprints.len(), 2);
    }

    #[test]
    fn test_avg_errors() {
        let history = QueryFeedbackHistory::new(10);

        // Add two pieces of feedback that include estimated errors.
        let mut feedback1 = QueryExecutionFeedback::new("fp_123".to_string());
        feedback1.estimated_rows = 100;
        feedback1.actual_rows = 110; // 10% error
        history.add_feedback(feedback1);

        let mut feedback2 = QueryExecutionFeedback::new("fp_123".to_string());
        feedback2.estimated_rows = 100;
        feedback2.actual_rows = 90; // 10% error
        history.add_feedback(feedback2);

        let avg_error = history
            .get_avg_row_error("fp_123")
            .expect("get_avg_row_error should succeed");
        assert!((avg_error - 0.1).abs() < 0.01); // The average error should be close to 0.1
    }

    #[test]
    fn test_nonexistent_query() {
        let history = QueryFeedbackHistory::new(10);

        assert_eq!(history.get_feedback_count("nonexistent"), 0);
        assert!(history.get_feedback_for_query("nonexistent").is_empty());
        assert!(history.get_avg_row_error("nonexistent").is_none());
    }
}
