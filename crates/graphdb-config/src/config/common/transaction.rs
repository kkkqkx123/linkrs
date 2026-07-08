//! Transaction configuration

use serde::{Deserialize, Serialize};

/// Transaction configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransactionConfig {
    /// Default transaction timeout (seconds)
    pub default_timeout: u64,
    /// Maximum concurrent transactions
    pub max_concurrent_transactions: usize,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            default_timeout: 30,
            max_concurrent_transactions: 1000,
        }
    }
}

impl TransactionConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_concurrent_transactions == 0 {
            return Err("Max concurrent transactions must be greater than 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_config_default() {
        let config = TransactionConfig::default();
        assert_eq!(config.default_timeout, 30);
        assert_eq!(config.max_concurrent_transactions, 1000);
    }

    #[test]
    fn test_transaction_config_validate() {
        let config = TransactionConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = TransactionConfig {
            max_concurrent_transactions: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
