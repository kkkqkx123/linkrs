//! Query Feedback Structure Module
//!
//! A data structure that provides feedback on the execution of a query, including feedback on the operators used and details regarding the query execution itself.

use std::time::Instant;

/// Operator execution feedback
///
/// Records statistical information about the execution of individual operators.
/// Refer to the output format of EXPLAIN ANALYZE in PostgreSQL.
///
/// # Field Description
/// `operator_id`: The unique identifier for the operator.
/// `operator_type`: The type of the operator (such as Scan, Filter, Join, etc.)
/// `estimated_rows`: The number of output rows estimated by the optimizer.
/// `actual_rows`: The actual number of rows output.
/// estimated_time_us: Estimated execution time (in microseconds)
/// `actual_time_us`: Actual execution time (in microseconds)
/// `execution_loops`: The number of executions (for example, the number of times the inner loop of a Nested Loop is executed).
///
/// # Example
/// ```
/// use graphdb::query::optimizer::stats::feedback::query::OperatorFeedback;
///
/// let feedback = OperatorFeedback {
///     operator_id: "scan_1".to_string(),
///     operator_type: "IndexScan".to_string(),
///     estimated_rows: 100,
///     actual_rows: 150,
///     estimated_time_us: 1000,
///     actual_time_us: 1200,
///     execution_loops: 1,
/// };
///
/// assert_eq!(feedback.row_estimation_error(), 0.5); // (150-100)/100
/// ```
#[derive(Debug, Clone)]
pub struct OperatorFeedback {
    /// Operator ID
    pub operator_id: String,
    /// Operator types
    pub operator_type: String,
    /// Estimated number of output lines
    pub estimated_rows: u64,
    /// Number of actual lines of output
    pub actual_rows: u64,
    /// Estimated execution time (in microseconds)
    pub estimated_time_us: u64,
    /// Actual execution time (in microseconds)
    pub actual_time_us: u64,
    /// Number of executions (for example, the number of times the inner loop of a nested loop is executed)
    pub execution_loops: u64,
}

impl OperatorFeedback {
    /// Create new operator feedback.
    pub fn new(
        operator_id: String,
        operator_type: String,
        estimated_rows: u64,
        actual_rows: u64,
    ) -> Self {
        Self {
            operator_id,
            operator_type,
            estimated_rows,
            actual_rows,
            estimated_time_us: 0,
            actual_time_us: 0,
            execution_loops: 1,
        }
    }

    /// Estimation error of the number of lines calculated
    ///
    /// Return the relative error: (Actual – Estimated) / Estimated
    /// If the estimate is 0, return 1.0
    pub fn row_estimation_error(&self) -> f64 {
        if self.estimated_rows == 0 {
            return 1.0;
        }
        let estimated = self.estimated_rows as f64;
        let actual = self.actual_rows as f64;
        ((actual - estimated).abs() / estimated).min(10.0)
    }

    /// Calculating the estimated error in time measurement
    ///
    /// Return relative error: (actual-estimate)/estimate
    pub fn time_estimation_error(&self) -> f64 {
        if self.estimated_time_us == 0 {
            return 1.0;
        }
        let estimated = self.estimated_time_us as f64;
        let actual = self.actual_time_us as f64;
        ((actual - estimated).abs() / estimated).min(10.0)
    }

    /// Obtain the average number of actual rows executed in each case.
    ///
    /// For operators that are executed multiple times (such as the inner loop in a Nested Loop),
    /// Return the average number of lines executed per run.
    pub fn avg_rows_per_loop(&self) -> f64 {
        if self.execution_loops == 0 {
            return 0.0;
        }
        self.actual_rows as f64 / self.execution_loops as f64
    }

    /// Obtain the average actual time (in microseconds) for each execution.
    pub fn avg_time_us_per_loop(&self) -> f64 {
        if self.execution_loops == 0 {
            return 0.0;
        }
        self.actual_time_us as f64 / self.execution_loops as f64
    }
}

/// Query execution feedback
///
/// Record the complete feedback information for each query execution.
///
/// # Examples
/// ```
/// use graphdb::query::optimizer::stats::feedback::query::QueryExecutionFeedback;
///
/// let mut feedback = QueryExecutionFeedback::new("query_fp_123".to_string());
/// feedback.estimated_rows = 1000;
/// feedback.actual_rows = 1200;
/// feedback.estimated_time_us = 5000;
/// feedback.actual_time_us = 6000;
///
/// assert!(feedback.row_estimation_error() > 0.0);
/// ```
#[derive(Debug, Clone)]
pub struct QueryExecutionFeedback {
    /// Querying fingerprints
    pub query_fingerprint: String,
    /// Estimated number of output lines
    pub estimated_rows: u64,
    /// Actual number of output lines
    pub actual_rows: u64,
    /// Estimated execution time (microseconds)
    pub estimated_time_us: u64,
    /// Actual execution time (microseconds)
    pub actual_time_us: u64,
    /// Execution timestamp
    pub execution_timestamp: Instant,
    /// Feedback from each operator
    pub operator_feedbacks: Vec<OperatorFeedback>,
}

impl QueryExecutionFeedback {
    /// Create new feedback for query execution.
    pub fn new(query_fingerprint: String) -> Self {
        Self {
            query_fingerprint,
            estimated_rows: 0,
            actual_rows: 0,
            estimated_time_us: 0,
            actual_time_us: 0,
            execution_timestamp: Instant::now(),
            operator_feedbacks: Vec::new(),
        }
    }

    /// Estimated error in calculating the number of rows
    pub fn row_estimation_error(&self) -> f64 {
        if self.estimated_rows == 0 {
            return 1.0;
        }
        let estimated = self.estimated_rows as f64;
        let actual = self.actual_rows as f64;
        ((actual - estimated).abs() / estimated).min(10.0)
    }

    /// Calculation time estimation error
    pub fn time_estimation_error(&self) -> f64 {
        if self.estimated_time_us == 0 {
            return 1.0;
        }
        let estimated = self.estimated_time_us as f64;
        let actual = self.actual_time_us as f64;
        ((actual - estimated).abs() / estimated).min(10.0)
    }

    /// Add operator feedback
    pub fn add_operator_feedback(&mut self, feedback: OperatorFeedback) {
        self.operator_feedbacks.push(feedback);
    }

    /// Number of operator feedbacks obtained
    pub fn operator_feedback_count(&self) -> usize {
        self.operator_feedbacks.len()
    }

    /// Obtaining feedback on a specific operator
    pub fn get_operator_feedback(&self, operator_id: &str) -> Option<&OperatorFeedback> {
        self.operator_feedbacks
            .iter()
            .find(|f| f.operator_id == operator_id)
    }

    /// Calculate the average estimation error for the number of rows of all operators.
    pub fn avg_operator_row_error(&self) -> f64 {
        if self.operator_feedbacks.is_empty() {
            return 0.0;
        }
        let total_error: f64 = self
            .operator_feedbacks
            .iter()
            .map(|f| f.row_estimation_error())
            .sum();
        total_error / self.operator_feedbacks.len() as f64
    }

    /// Calculate the average time estimation error for all operators.
    pub fn avg_operator_time_error(&self) -> f64 {
        if self.operator_feedbacks.is_empty() {
            return 0.0;
        }
        let total_error: f64 = self
            .operator_feedbacks
            .iter()
            .map(|f| f.time_estimation_error())
            .sum();
        total_error / self.operator_feedbacks.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operator_feedback() {
        let feedback = OperatorFeedback {
            operator_id: "scan_1".to_string(),
            operator_type: "IndexScan".to_string(),
            estimated_rows: 100,
            actual_rows: 150,
            estimated_time_us: 1000,
            actual_time_us: 1200,
            execution_loops: 1,
        };

        assert_eq!(feedback.row_estimation_error(), 0.5); // (150-100)/100
        assert_eq!(feedback.time_estimation_error(), 0.2); // (1200-1000)/1000
    }

    #[test]
    fn test_operator_feedback_loops() {
        let feedback = OperatorFeedback {
            operator_id: "nested_loop_inner".to_string(),
            operator_type: "IndexScan".to_string(),
            estimated_rows: 100,
            actual_rows: 500, // In total, 500 lines were processed, and this operation was performed 10 times.
            estimated_time_us: 1000,
            actual_time_us: 5000,
            execution_loops: 10,
        };

        assert_eq!(feedback.avg_rows_per_loop(), 50.0); // 500/10
        assert_eq!(feedback.avg_time_us_per_loop(), 500.0); // 5000/10
    }

    #[test]
    fn test_query_execution_feedback() {
        let mut feedback = QueryExecutionFeedback::new("fp_123".to_string());
        feedback.estimated_rows = 1000;
        feedback.actual_rows = 1200;
        feedback.estimated_time_us = 5000;
        feedback.actual_time_us = 6000;

        // Add operator feedback
        let op_feedback =
            OperatorFeedback::new("scan_1".to_string(), "SeqScan".to_string(), 1000, 1200);
        feedback.add_operator_feedback(op_feedback);

        assert_eq!(feedback.operator_feedback_count(), 1);
        assert!(feedback.row_estimation_error() > 0.0);
        assert!(feedback.time_estimation_error() > 0.0);
    }

    #[test]
    fn test_avg_operator_errors() {
        let mut feedback = QueryExecutionFeedback::new("fp_123".to_string());

        // Add feedback for two operators.
        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op1".to_string(),
            operator_type: "Scan".to_string(),
            estimated_rows: 100,
            actual_rows: 110,
            estimated_time_us: 1000,
            actual_time_us: 1100,
            execution_loops: 1,
        });

        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op2".to_string(),
            operator_type: "Filter".to_string(),
            estimated_rows: 100,
            actual_rows: 90,
            estimated_time_us: 500,
            actual_time_us: 450,
            execution_loops: 1,
        });

        let avg_row_error = feedback.avg_operator_row_error();
        let avg_time_error = feedback.avg_operator_time_error();

        assert!(avg_row_error > 0.0);
        assert!(avg_time_error > 0.0);
    }
}
