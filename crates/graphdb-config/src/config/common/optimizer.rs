//! Optimizer configuration

use serde::{Deserialize, Serialize};

/// Optimizer rules configuration
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct OptimizerRulesConfig {
    /// Disabled rules
    #[serde(default)]
    pub disabled_rules: Vec<String>,
    /// Enabled rules
    #[serde(default)]
    pub enabled_rules: Vec<String>,
}

/// Optimizer configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OptimizerConfig {
    /// Maximum iteration rounds
    pub max_iteration_rounds: usize,
    /// Maximum exploration rounds
    pub max_exploration_rounds: usize,
    /// Whether to enable cost model
    pub enable_cost_model: bool,
    /// Whether to enable multi-plan
    pub enable_multi_plan: bool,
    /// Whether to enable property pruning
    pub enable_property_pruning: bool,
    /// Whether to enable adaptive iteration
    pub enable_adaptive_iteration: bool,
    /// Stable threshold
    pub stable_threshold: usize,
    /// Minimum iteration rounds
    pub min_iteration_rounds: usize,
    /// Rules configuration
    #[serde(default)]
    pub rules: OptimizerRulesConfig,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            max_iteration_rounds: 5,
            max_exploration_rounds: 128,
            enable_cost_model: true,
            enable_multi_plan: true,
            enable_property_pruning: true,
            enable_adaptive_iteration: true,
            stable_threshold: 2,
            min_iteration_rounds: 1,
            rules: OptimizerRulesConfig::default(),
        }
    }
}

impl OptimizerConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_iteration_rounds == 0 {
            return Err("Max iteration rounds must be greater than 0".to_string());
        }

        if self.max_exploration_rounds == 0 {
            return Err("Max exploration rounds must be greater than 0".to_string());
        }

        if self.min_iteration_rounds > self.max_iteration_rounds {
            return Err(
                "Min iteration rounds cannot be greater than max iteration rounds".to_string(),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_config_default() {
        let config = OptimizerConfig::default();
        assert_eq!(config.max_iteration_rounds, 5);
        assert_eq!(config.max_exploration_rounds, 128);
        assert!(config.enable_cost_model);
        assert!(config.enable_multi_plan);
    }

    #[test]
    fn test_optimizer_config_validate() {
        let config = OptimizerConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = OptimizerConfig {
            max_iteration_rounds: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
