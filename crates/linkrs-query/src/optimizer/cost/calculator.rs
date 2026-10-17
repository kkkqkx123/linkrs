//! Cost Calculator Module
//!
//! Lightweight cost calculation designed for the characteristics of graph databases
//!
//! ## Usage Examples
//!
//! ```rust
//! use linkrs_query::optimizer::cost::{CostCalculator, CostModelConfig};
//! use linkrs_query::optimizer::stats::StatisticsManager;
//! use std::sync::Arc;
//!
//! let stats_manager = Arc::new(StatisticsManager::new());
//! let config = CostModelConfig::default();
//! let calculator = CostCalculator::with_config(stats_manager, config);
//!
//! // Calculate the scanning cost
//! let scan_cost = calculator.calculate_scan_vertices_cost("my_space", "Person");
//! ```

use std::sync::Arc;

use super::config::CostModelConfig;
use crate::optimizer::stats::StatisticsManager;

/// Cost Calculator
///
/// Lightweight cost calculation designed for the characteristics of graph databases
#[derive(Debug, Clone)]
pub struct CostCalculator {
    stats_manager: Arc<StatisticsManager>,
    config: CostModelConfig,
}

impl CostCalculator {
    /// Create a new cost calculator (using the default configuration).
    pub fn new(stats_manager: Arc<StatisticsManager>) -> Self {
        Self {
            stats_manager,
            config: CostModelConfig::default(),
        }
    }

    /// Create a new cost calculator (using the specified configuration).
    pub fn with_config(stats_manager: Arc<StatisticsManager>, config: CostModelConfig) -> Self {
        Self {
            stats_manager,
            config,
        }
    }

    /// Obtain the configuration.
    pub fn config(&self) -> &CostModelConfig {
        &self.config
    }

    /// Update the configuration.
    pub fn set_config(&mut self, config: CostModelConfig) {
        self.config = config;
    }
}

mod aggregate_sort;
mod helpers;
mod join;
mod scan;
mod traverse;

impl Default for CostCalculator {
    fn default() -> Self {
        Self {
            stats_manager: Arc::new(StatisticsManager::new()),
            config: CostModelConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests;
