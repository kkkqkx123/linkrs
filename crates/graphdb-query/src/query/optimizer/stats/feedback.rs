//! Runtime statistics feedback module
//!
//! Provide a lightweight mechanism for collecting execution feedback, which can be used to dynamically adjust selective estimation models.
//! 使用指数加权移动平均(EWMA)算法校正选择性估计。
//!
//! ## Module Structure
//!
//! “Fingerprint” – Refers to the process of generating and normalizing fingerprints (digital representations of unique patterns).
//! “Collector” – The component responsible for executing the feedback collection process.
//! “Trigger” – A mechanism for automatically triggering feedback.
//! “Selectivity” refers to the process of carefully selecting and applying appropriate corrections or measures in a targeted manner. In other words, it involves managing various factors or elements in a way that is tailored to achieve specific goals or outcomes.
//! `query` – The structure that contains feedback regarding the execution of the query.
//! “History” – Querying the feedback history management system.

pub mod collector;
pub mod fingerprint;
pub mod history;
pub mod query;
pub mod selectivity;
pub mod trigger;

// Re-export the main types while maintaining backward compatibility.
pub use collector::{ExecutionFeedbackCollector, SimpleExecutionFeedback, SimpleFeedbackCollector};
pub use fingerprint::{generate_query_fingerprint, normalize_query};
pub use history::QueryFeedbackHistory;
pub use query::{OperatorFeedback, QueryExecutionFeedback};
pub use selectivity::{FeedbackDrivenSelectivity, SelectivityFeedbackManager};
pub use trigger::{AutoFeedbackConfig, AutoFeedbackTrigger};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_integration() {
        // Integration of the testing module
        let collector = ExecutionFeedbackCollector::new();
        collector.start();
        collector.record_rows(100);
        collector.finish();

        let mut selectivity = FeedbackDrivenSelectivity::new(0.1);
        selectivity.update_with_feedback(0.15);

        let query_feedback = QueryExecutionFeedback::new("fp_123".to_string());

        let history = QueryFeedbackHistory::new(10);
        // Add feedback to verify that the “history” functionality is working properly.
        history.add_feedback(query_feedback.clone());

        let config = AutoFeedbackConfig::new();

        // All modules are functioning properly.
        assert_eq!(collector.get_actual_rows(), 100);
        assert!(selectivity.corrected_selectivity() > 0.0);
        assert_eq!(query_feedback.query_fingerprint, "fp_123");
        assert_eq!(history.total_feedback_count(), 1);
        assert!(config.enabled);
    }
}
