//! Database configuration

use serde::{Deserialize, Serialize};

/// Database configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseConfig {
    /// Host address
    pub host: String,
    /// Port
    pub port: u16,
    /// Storage path
    pub storage_path: String,
    /// Maximum connections
    pub max_connections: usize,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 9758,
            storage_path: "data/graphdb".to_string(),
            max_connections: 10,
        }
    }
}

impl DatabaseConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("Database port cannot be 0".to_string());
        }

        if self.max_connections == 0 {
            return Err("Max connections must be greater than 0".to_string());
        }

        if self.storage_path.is_empty() {
            return Err("Storage path cannot be empty".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_config_default() {
        let config = DatabaseConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9758);
        assert_eq!(config.storage_path, "data/graphdb");
        assert_eq!(config.max_connections, 10);
    }

    #[test]
    fn test_database_config_validate() {
        let config = DatabaseConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = DatabaseConfig {
            port: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        let invalid_config = DatabaseConfig {
            max_connections: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
