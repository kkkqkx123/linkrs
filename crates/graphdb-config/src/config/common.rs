//! Common configuration modules
//!
//! Contains configuration that is shared across all usage patterns (server, embedded, c-api)

pub mod database;
pub mod fulltext;
pub mod log;
pub mod monitoring;
pub mod optimizer;
pub mod storage;
pub mod transaction;

pub use database::*;
pub use fulltext::*;
pub use log::*;
pub use monitoring::*;
pub use optimizer::*;
pub use storage::*;
pub use transaction::*;

use serde::{Deserialize, Serialize};

/// Common configuration aggregator
///
/// Contains all configuration that is shared across different usage patterns.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct CommonConfig {
    /// Database configuration
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Transaction configuration
    #[serde(default)]
    pub transaction: TransactionConfig,

    /// Log configuration
    #[serde(default)]
    pub log: LogConfig,

    /// Storage configuration
    #[serde(default)]
    pub storage: StorageConfig,

    /// Optimizer configuration
    #[serde(default)]
    pub optimizer: OptimizerConfig,

    /// Monitoring configuration
    #[serde(default)]
    pub monitoring: MonitoringConfig,

    /// Query resource configuration
    #[serde(default)]
    pub query_resource: QueryResourceConfig,
}

impl CommonConfig {
    /// Create a new common configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate all common configurations
    pub fn validate(&self) -> Result<(), String> {
        self.database.validate()?;
        self.transaction.validate()?;
        self.log.validate()?;
        self.storage.validate()?;
        self.optimizer.validate()?;
        self.monitoring.validate()?;
        self.query_resource.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_config_default() {
        let config = CommonConfig::default();
        assert_eq!(config.database.host, "127.0.0.1");
        assert_eq!(config.database.port, 9758);
        assert_eq!(config.log.level, "info");
        assert_eq!(config.optimizer.max_iteration_rounds, 5);
    }

    #[test]
    fn test_common_config_validate() {
        let config = CommonConfig::default();
        assert!(config.validate().is_ok());
    }
}
