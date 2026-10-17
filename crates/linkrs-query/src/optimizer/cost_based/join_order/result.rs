//! Result types of join order optimization.

use crate::optimizer::decision::JoinAlgorithm;

/// Results of the optimization of the connection sequence
#[derive(Debug, Clone)]
pub struct JoinOrderResult {
    /// Optimal connection order (list of table IDs)
    pub order: Vec<String>,
    /// The choice of algorithm for each connection
    pub algorithms: Vec<JoinAlgorithm>,
    /// Total estimated cost
    pub total_cost: f64,
    /// Final estimated number of output lines
    pub final_output_rows: u64,
    /// The optimization algorithms used
    pub optimization_method: OptimizationMethod,
}

/// Optimization methods
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationMethod {
    /// Dynamic Programming
    DynamicProgramming,
    /// Greedy algorithm
    Greedy,
    /// Heuristic approach (too few examples available)
    Heuristic,
}
