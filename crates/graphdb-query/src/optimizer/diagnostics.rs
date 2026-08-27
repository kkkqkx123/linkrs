//! Optimizer Diagnostics Module
//!
//! Provides diagnostics information for optimizer decisions, including:
//! - Optimization statistics (iterations, rules applied, batch results)
//! - Physical choice decisions (scan/index, join, aggregate)
//! - Statistics version, estimated rows, cost, and decision reasons
//!
//! This information is used by EXPLAIN to show optimization details.

use crate::optimizer::heuristic::{BatchStopReason, OptimizationResult};

/// Physical choice for a specific operation
#[derive(Debug, Clone)]
pub enum PhysicalChoice {
    /// Scan/index selection
    Scan {
        /// Index name if using index, None if full scan
        index_name: Option<String>,
        /// Estimated rows
        estimated_rows: f64,
        /// Estimated cost
        cost: f64,
        /// Decision reason
        reason: String,
    },
    /// Two-table join
    TwoTableJoin {
        /// Join algorithm (hash, merge, nested loop)
        algorithm: String,
        /// Left table name
        left_table: String,
        /// Right table name
        right_table: String,
        /// Estimated rows
        estimated_rows: f64,
        /// Estimated cost
        cost: f64,
        /// Decision reason
        reason: String,
    },
    /// Aggregate strategy
    Aggregate {
        /// Strategy name
        strategy: String,
        /// Estimated rows
        estimated_rows: f64,
        /// Estimated cost
        cost: f64,
        /// Decision reason
        reason: String,
    },
}

/// Optimizer diagnostics for EXPLAIN
#[derive(Debug, Clone)]
pub struct OptimizerDiagnostics {
    /// Statistics version
    pub statistics_version: u64,
    /// Optimization phases
    pub phases: Vec<PhaseDiagnostics>,
    /// Physical choices made
    pub physical_choices: Vec<PhysicalChoice>,
    /// Total estimated rows
    pub total_estimated_rows: f64,
    /// Total estimated cost
    pub total_estimated_cost: f64,
    /// Whether cost-based optimization was used
    pub cost_based_used: bool,
}

/// Diagnostics for a single optimization phase
#[derive(Debug, Clone)]
pub struct PhaseDiagnostics {
    /// Phase name (heuristic, cost-based)
    pub phase_name: String,
    /// Whether the phase was enabled
    pub enabled: bool,
    /// Batch statistics
    pub batch_stats: Vec<BatchDiagnostics>,
    /// Total iterations
    pub total_iterations: usize,
    /// Total rules applied
    pub total_rules_applied: usize,
}

/// Diagnostics for a single optimization batch
#[derive(Debug, Clone)]
pub struct BatchDiagnostics {
    /// Batch name
    pub batch_name: String,
    /// Iterations performed
    pub iterations: usize,
    /// Rules applied
    pub rules_applied: usize,
    /// Rule hit counts
    pub rule_hit_counts: Vec<(String, usize)>,
    /// Whether converged
    pub converged: bool,
    /// Stop reason
    pub stop_reason: String,
    /// Whether oscillation was detected
    pub oscillation_detected: bool,
}

impl OptimizerDiagnostics {
    /// Create new diagnostics from optimization result
    pub fn from_optimization_result(
        result: &OptimizationResult,
        statistics_version: u64,
        cost_based_used: bool,
    ) -> Self {
        let mut phase_stats = PhaseDiagnostics {
            phase_name: "heuristic".to_string(),
            enabled: true,
            batch_stats: Vec::new(),
            total_iterations: result.total_iterations,
            total_rules_applied: result.total_rules_applied,
        };

        for (batch, stats) in &result.batch_statistics {
            let diagnostics = BatchDiagnostics {
                batch_name: batch.name().to_string(),
                iterations: stats.iterations,
                rules_applied: stats.rules_applied,
                rule_hit_counts: stats
                    .rule_hit_counts
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect(),
                converged: stats.converged,
                stop_reason: format!("{:?}", stats.stop_reason),
                oscillation_detected: matches!(stats.stop_reason, BatchStopReason::CycleDetected),
            };
            phase_stats.batch_stats.push(diagnostics);
        }

        Self {
            statistics_version,
            phases: vec![phase_stats],
            physical_choices: Vec::new(),
            total_estimated_rows: 0.0,
            total_estimated_cost: 0.0,
            cost_based_used,
        }
    }

    /// Add a physical choice
    pub fn add_physical_choice(&mut self, choice: PhysicalChoice) {
        match &choice {
            PhysicalChoice::Scan {
                estimated_rows,
                cost,
                ..
            }
            | PhysicalChoice::TwoTableJoin {
                estimated_rows,
                cost,
                ..
            }
            | PhysicalChoice::Aggregate {
                estimated_rows,
                cost,
                ..
            } => {
                self.total_estimated_rows += estimated_rows;
                self.total_estimated_cost += cost;
            }
        }
        self.physical_choices.push(choice);
    }

    /// Generate EXPLAIN-compatible description
    pub fn describe(&self) -> String {
        let mut desc = String::new();

        desc.push_str(&format!(
            "Optimizer Statistics Version: {}\n",
            self.statistics_version
        ));
        desc.push_str(&format!(
            "Total Estimated Rows: {:.0}\n",
            self.total_estimated_rows
        ));
        desc.push_str(&format!(
            "Total Estimated Cost: {:.2}\n",
            self.total_estimated_cost
        ));
        desc.push_str(&format!(
            "Cost-Based Optimization: {}\n",
            if self.cost_based_used {
                "enabled"
            } else {
                "disabled"
            }
        ));

        for phase in &self.phases {
            desc.push_str(&format!("\nPhase: {}\n", phase.phase_name));
            desc.push_str(&format!(
                "  Total Iterations: {} | Total Rules Applied: {}\n",
                phase.total_iterations, phase.total_rules_applied
            ));

            for batch in &phase.batch_stats {
                desc.push_str(&format!(
                    "  Batch: {} | Iterations: {} | Rules Applied: {} | Converged: {} | Stop Reason: {}\n",
                    batch.batch_name,
                    batch.iterations,
                    batch.rules_applied,
                    batch.converged,
                    batch.stop_reason
                ));

                if batch.oscillation_detected {
                    desc.push_str(&format!(
                        "    ⚠ Oscillation detected in batch {}\n",
                        batch.batch_name
                    ));
                }

                for (rule_name, count) in &batch.rule_hit_counts {
                    if *count > 0 {
                        desc.push_str(&format!("    {}: {} hits\n", rule_name, count));
                    }
                }
            }
        }

        if !self.physical_choices.is_empty() {
            desc.push_str("\nPhysical Choices:\n");
            for choice in &self.physical_choices {
                match choice {
                    PhysicalChoice::Scan {
                        index_name,
                        estimated_rows,
                        cost,
                        reason,
                    } => {
                        desc.push_str(&format!(
                            "  Scan{}: est_rows={:.0}, cost={:.2}, reason={}\n",
                            if let Some(name) = index_name {
                                format!(" (index: {})", name)
                            } else {
                                String::new()
                            },
                            estimated_rows,
                            cost,
                            reason
                        ));
                    }
                    PhysicalChoice::TwoTableJoin {
                        algorithm,
                        left_table,
                        right_table,
                        estimated_rows,
                        cost,
                        reason,
                    } => {
                        desc.push_str(&format!(
                            "  Join({}): {} ⨝ {} est_rows={:.0}, cost={:.2}, reason={}\n",
                            algorithm, left_table, right_table, estimated_rows, cost, reason
                        ));
                    }
                    PhysicalChoice::Aggregate {
                        strategy,
                        estimated_rows,
                        cost,
                        reason,
                    } => {
                        desc.push_str(&format!(
                            "  Aggregate({}): est_rows={:.0}, cost={:.2}, reason={}\n",
                            strategy, estimated_rows, cost, reason
                        ));
                    }
                }
            }
        }

        desc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::heuristic::{
        BatchStatistics, BatchStopReason, OptimizationBatch, OptimizationResult,
    };
    use crate::planning::plan::core::nodes::StartNode;
    use crate::planning::plan::core::PlanNodeEnum;

    #[test]
    fn test_diagnostics_creation() {
        let result = OptimizationResult {
            optimized_plan: PlanNodeEnum::Start(StartNode::new()),
            batch_statistics: vec![(
                OptimizationBatch::Normalize,
                BatchStatistics {
                    iterations: 3,
                    rules_applied: 5,
                    converged: true,
                    stop_reason: BatchStopReason::Converged,
                    ..Default::default()
                },
            )],
            total_iterations: 3,
            total_rules_applied: 5,
        };

        let diagnostics = OptimizerDiagnostics::from_optimization_result(&result, 1, false);

        assert_eq!(diagnostics.statistics_version, 1);
        assert_eq!(diagnostics.phases.len(), 1);
        assert_eq!(diagnostics.total_estimated_rows, 0.0);
        assert!(!diagnostics.cost_based_used);
    }

    #[test]
    fn test_physical_choices() {
        let mut diagnostics = OptimizerDiagnostics {
            statistics_version: 1,
            phases: Vec::new(),
            physical_choices: Vec::new(),
            total_estimated_rows: 0.0,
            total_estimated_cost: 0.0,
            cost_based_used: false,
        };

        diagnostics.add_physical_choice(PhysicalChoice::Scan {
            index_name: Some("vertex_idx".to_string()),
            estimated_rows: 100.0,
            cost: 10.0,
            reason: "index selectivity < 0.1".to_string(),
        });

        diagnostics.add_physical_choice(PhysicalChoice::TwoTableJoin {
            algorithm: "hash".to_string(),
            left_table: "users".to_string(),
            right_table: "posts".to_string(),
            estimated_rows: 50.0,
            cost: 25.0,
            reason: "both tables have statistics".to_string(),
        });

        assert_eq!(diagnostics.physical_choices.len(), 2);
        assert!(diagnostics.total_estimated_rows > 0.0);
        assert!(diagnostics.total_estimated_cost > 0.0);

        let desc = diagnostics.describe();
        assert!(desc.contains("vertex_idx"));
        assert!(desc.contains("hash"));
        assert!(desc.contains("users"));
    }

    #[test]
    fn test_describe_oscillation_detection() {
        let result = OptimizationResult {
            optimized_plan: PlanNodeEnum::Start(StartNode::new()),
            batch_statistics: vec![(
                OptimizationBatch::PredicatePushdown,
                BatchStatistics {
                    iterations: 10,
                    rules_applied: 20,
                    converged: false,
                    stop_reason: BatchStopReason::CycleDetected,
                    ..Default::default()
                },
            )],
            total_iterations: 10,
            total_rules_applied: 20,
        };

        let diagnostics = OptimizerDiagnostics::from_optimization_result(&result, 1, false);

        let desc = diagnostics.describe();
        assert!(desc.contains("Oscillation detected"));
        assert!(desc.contains("CycleDetected"));
    }
}
