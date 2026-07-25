//! Physical Choice Decision Tracking
//!
//! This module provides physical choice decision tracking for the cost-based optimizer.
//! Each decision records:
//! - Statistics version
//! - Estimated rows
//! - Estimated cost
//! - Decision reason
//!
//! These decisions are exposed through EXPLAIN for diagnostics.

use crate::query::optimizer::diagnostics::PhysicalChoice;

/// Physical choice decision for scan/index operations
#[derive(Debug, Clone)]
pub struct ScanDecision {
    /// Statistics version used for the decision
    pub statistics_version: u64,
    /// Index name if using index, None if full scan
    pub index_name: Option<String>,
    /// Estimated rows
    pub estimated_rows: f64,
    /// Estimated cost
    pub cost: f64,
    /// Decision reason
    pub reason: String,
}

/// Physical choice decision for join operations
#[derive(Debug, Clone)]
pub struct JoinDecision {
    /// Statistics version used for the decision
    pub statistics_version: u64,
    /// Join algorithm
    pub algorithm: String,
    /// Left table name
    pub left_table: String,
    /// Right table name
    pub right_table: String,
    /// Estimated rows
    pub estimated_rows: f64,
    /// Estimated cost
    pub cost: f64,
    /// Decision reason
    pub reason: String,
}

/// Physical choice decision for aggregate operations
#[derive(Debug, Clone)]
pub struct AggregateDecision {
    /// Statistics version used for the decision
    pub statistics_version: u64,
    /// Aggregate strategy
    pub strategy: String,
    /// Estimated rows
    pub estimated_rows: f64,
    /// Estimated cost
    pub cost: f64,
    /// Decision reason
    pub reason: String,
}

/// Physical choice decision tracker
#[derive(Debug, Clone, Default)]
pub struct PhysicalChoiceTracker {
    /// Scan/index decisions
    pub scan_decisions: Vec<ScanDecision>,
    /// Join decisions
    pub join_decisions: Vec<JoinDecision>,
    /// Aggregate decisions
    pub aggregate_decisions: Vec<AggregateDecision>,
    /// Statistics version
    pub statistics_version: u64,
}

impl PhysicalChoiceTracker {
    /// Create a new tracker
    pub fn new(statistics_version: u64) -> Self {
        Self {
            scan_decisions: Vec::new(),
            join_decisions: Vec::new(),
            aggregate_decisions: Vec::new(),
            statistics_version,
        }
    }

    /// Record a scan/index decision
    pub fn record_scan(
        &mut self,
        index_name: Option<String>,
        estimated_rows: f64,
        cost: f64,
        reason: String,
    ) {
        self.scan_decisions.push(ScanDecision {
            statistics_version: self.statistics_version,
            index_name,
            estimated_rows,
            cost,
            reason,
        });
    }

    /// Record a join decision
    pub fn record_join(
        &mut self,
        algorithm: String,
        left_table: String,
        right_table: String,
        estimated_rows: f64,
        cost: f64,
        reason: String,
    ) {
        self.join_decisions.push(JoinDecision {
            statistics_version: self.statistics_version,
            algorithm,
            left_table,
            right_table,
            estimated_rows,
            cost,
            reason,
        });
    }

    /// Record an aggregate decision
    pub fn record_aggregate(
        &mut self,
        strategy: String,
        estimated_rows: f64,
        cost: f64,
        reason: String,
    ) {
        self.aggregate_decisions.push(AggregateDecision {
            statistics_version: self.statistics_version,
            strategy,
            estimated_rows,
            cost,
            reason,
        });
    }

    /// Convert all decisions to physical choices for EXPLAIN
    pub fn to_physical_choices(&self) -> Vec<PhysicalChoice> {
        let mut choices = Vec::new();

        for decision in &self.scan_decisions {
            choices.push(PhysicalChoice::Scan {
                index_name: decision.index_name.clone(),
                estimated_rows: decision.estimated_rows,
                cost: decision.cost,
                reason: format!(
                    "[stats_v{}] {}",
                    decision.statistics_version, decision.reason
                ),
            });
        }

        for decision in &self.join_decisions {
            choices.push(PhysicalChoice::TwoTableJoin {
                algorithm: decision.algorithm.clone(),
                left_table: decision.left_table.clone(),
                right_table: decision.right_table.clone(),
                estimated_rows: decision.estimated_rows,
                cost: decision.cost,
                reason: format!(
                    "[stats_v{}] {}",
                    decision.statistics_version, decision.reason
                ),
            });
        }

        for decision in &self.aggregate_decisions {
            choices.push(PhysicalChoice::Aggregate {
                strategy: decision.strategy.clone(),
                estimated_rows: decision.estimated_rows,
                cost: decision.cost,
                reason: format!(
                    "[stats_v{}] {}",
                    decision.statistics_version, decision.reason
                ),
            });
        }

        choices
    }

    /// Check if any decisions were made
    pub fn has_decisions(&self) -> bool {
        !self.scan_decisions.is_empty()
            || !self.join_decisions.is_empty()
            || !self.aggregate_decisions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_creation() {
        let tracker = PhysicalChoiceTracker::new(1);
        assert_eq!(tracker.statistics_version, 1);
        assert!(!tracker.has_decisions());
    }

    #[test]
    fn test_record_scan_decision() {
        let mut tracker = PhysicalChoiceTracker::new(1);
        tracker.record_scan(
            Some("vertex_idx".to_string()),
            100.0,
            10.0,
            "index selectivity < 0.1".to_string(),
        );

        assert!(tracker.has_decisions());
        assert_eq!(tracker.scan_decisions.len(), 1);
        assert_eq!(tracker.scan_decisions[0].estimated_rows, 100.0);
    }

    #[test]
    fn test_record_join_decision() {
        let mut tracker = PhysicalChoiceTracker::new(2);
        tracker.record_join(
            "hash".to_string(),
            "users".to_string(),
            "posts".to_string(),
            50.0,
            25.0,
            "both tables have statistics".to_string(),
        );

        assert!(tracker.has_decisions());
        assert_eq!(tracker.join_decisions.len(), 1);
        assert_eq!(tracker.join_decisions[0].algorithm, "hash");
    }

    #[test]
    fn test_record_aggregate_decision() {
        let mut tracker = PhysicalChoiceTracker::new(3);
        tracker.record_aggregate(
            "hash".to_string(),
            10.0,
            5.0,
            "group key cardinality < threshold".to_string(),
        );

        assert!(tracker.has_decisions());
        assert_eq!(tracker.aggregate_decisions.len(), 1);
        assert_eq!(tracker.aggregate_decisions[0].strategy, "hash");
    }

    #[test]
    fn test_to_physical_choices() {
        let mut tracker = PhysicalChoiceTracker::new(1);
        tracker.record_scan(None, 1000.0, 100.0, "full scan".to_string());
        tracker.record_join(
            "merge".to_string(),
            "a".to_string(),
            "b".to_string(),
            500.0,
            250.0,
            "sorted inputs".to_string(),
        );

        let choices = tracker.to_physical_choices();
        assert_eq!(choices.len(), 2);
    }
}
