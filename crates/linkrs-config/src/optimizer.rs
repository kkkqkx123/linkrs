//! Optimizer configuration

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Optimizer rules configuration
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    /// Statistics sampling window (rows per tag/edge type per collection).
    #[serde(default = "default_statistics_sample_limit")]
    pub statistics_sample_limit: usize,
    /// Minimum data-epoch advance that triggers an automatic refresh.
    ///
    /// Zero means every epoch advance refreshes; larger values let small
    /// write batches reuse the last collection.
    #[serde(default = "default_statistics_min_epoch_delta")]
    pub statistics_min_epoch_delta: u64,
    /// Storage cost profile selecting the optimizer cost preset.
    #[serde(default)]
    pub storage_cost_profile: crate::cost_profile::StorageCostProfile,
    /// Per-space storage cost profile overrides (space name -> profile).
    ///
    /// Spaces absent from the map use the global profile. Unknown space
    /// names are accepted and simply never match; memory-mode deployments
    /// short-circuit every space to the in-memory preset.
    #[serde(default)]
    pub space_cost_profiles: HashMap<String, crate::cost_profile::StorageCostProfile>,
    /// Rules configuration
    #[serde(default)]
    pub rules: OptimizerRulesConfig,
}

fn default_statistics_sample_limit() -> usize {
    10_000
}

fn default_statistics_min_epoch_delta() -> u64 {
    100
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
            statistics_sample_limit: default_statistics_sample_limit(),
            statistics_min_epoch_delta: default_statistics_min_epoch_delta(),
            storage_cost_profile: crate::cost_profile::StorageCostProfile::default(),
            space_cost_profiles: HashMap::new(),
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

        if self.statistics_sample_limit == 0 {
            return Err("Statistics sample limit must be greater than 0".to_string());
        }

        if self
            .space_cost_profiles
            .keys()
            .any(|name| name.trim().is_empty())
        {
            return Err("Space cost profile names must not be empty".to_string());
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
        assert_eq!(config.statistics_sample_limit, 10_000);
        assert_eq!(config.statistics_min_epoch_delta, 100);
        assert_eq!(
            config.storage_cost_profile,
            crate::cost_profile::StorageCostProfile::Auto
        );
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

    #[test]
    fn test_space_cost_profile_override_defaults_empty() {
        let config = OptimizerConfig::default();
        assert!(config.space_cost_profiles.is_empty());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_space_cost_profile_rejects_empty_name() {
        let mut config = OptimizerConfig::default();
        config.space_cost_profiles.insert(
            "  ".to_string(),
            crate::cost_profile::StorageCostProfile::Hdd,
        );
        assert!(config.validate().is_err());
    }
}
