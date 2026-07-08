//! Embedded configuration modules
//!
//! Contains configuration specific to embedded mode (library usage).
//! These configurations are only available when the `embedded` feature is enabled.

pub mod runtime;

pub use runtime::*;

use serde::{Deserialize, Serialize};

/// Embedded configuration aggregator
///
/// Contains all configuration specific to embedded mode.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct EmbeddedConfig {
    /// Runtime configuration
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

impl EmbeddedConfig {
    /// Create a new embedded configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate all embedded configurations
    pub fn validate(&self) -> Result<(), String> {
        self.runtime.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_config_default() {
        let config = EmbeddedConfig::default();
        assert!(config.validate().is_ok());
    }
}
