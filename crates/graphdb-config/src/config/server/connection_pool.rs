//! Connection pool configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Connection pool configuration
///
/// Controls connection pooling behavior for server mode.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ConnectionPoolConfig {
    /// Minimum idle connections
    pub min_idle: usize,
    /// Maximum connections
    pub max_size: usize,
    /// Connection timeout (seconds)
    pub connection_timeout_secs: u64,
    /// Idle timeout (seconds)
    pub idle_timeout_secs: u64,
    /// Maximum connection lifetime (seconds)
    pub max_lifetime_secs: u64,
    /// Connection health check interval (seconds, 0 = disabled)
    pub health_check_interval_secs: u64,
    /// Maximum connection age before recycling (seconds, 0 = disabled)
    pub max_age_secs: u64,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            min_idle: 5,
            max_size: 50,
            connection_timeout_secs: 30,
            idle_timeout_secs: 600,  // 10 minutes
            max_lifetime_secs: 1800, // 30 minutes
            health_check_interval_secs: 30,
            max_age_secs: 0, // Disabled
        }
    }
}

impl ConnectionPoolConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.min_idle > self.max_size {
            return Err("Min idle cannot be greater than max size".to_string());
        }

        if self.max_size == 0 {
            return Err("Max size must be greater than 0".to_string());
        }

        if self.connection_timeout_secs == 0 {
            return Err("Connection timeout must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Get connection timeout as Duration
    pub fn connection_timeout(&self) -> Duration {
        Duration::from_secs(self.connection_timeout_secs)
    }

    /// Get idle timeout as Duration
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs)
    }

    /// Get max lifetime as Duration
    pub fn max_lifetime(&self) -> Duration {
        Duration::from_secs(self.max_lifetime_secs)
    }

    /// Get health check interval as Duration
    pub fn health_check_interval(&self) -> Option<Duration> {
        if self.health_check_interval_secs > 0 {
            Some(Duration::from_secs(self.health_check_interval_secs))
        } else {
            None
        }
    }

    /// Get max age as Duration
    pub fn max_age(&self) -> Option<Duration> {
        if self.max_age_secs > 0 {
            Some(Duration::from_secs(self.max_age_secs))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_pool_config_default() {
        let config = ConnectionPoolConfig::default();
        assert_eq!(config.min_idle, 5);
        assert_eq!(config.max_size, 50);
        assert_eq!(config.connection_timeout_secs, 30);
        assert_eq!(config.idle_timeout_secs, 600);
        assert_eq!(config.max_lifetime_secs, 1800);
        assert_eq!(config.health_check_interval_secs, 30);
        assert_eq!(config.max_age_secs, 0);
    }

    #[test]
    fn test_connection_pool_config_validate() {
        let config = ConnectionPoolConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = ConnectionPoolConfig {
            min_idle: 100,
            max_size: 50,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        let invalid_config = ConnectionPoolConfig {
            max_size: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_connection_pool_config_durations() {
        let config = ConnectionPoolConfig::default();
        assert_eq!(config.connection_timeout(), Duration::from_secs(30));
        assert_eq!(config.idle_timeout(), Duration::from_secs(600));
        assert_eq!(config.max_lifetime(), Duration::from_secs(1800));
        assert_eq!(
            config.health_check_interval(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(config.max_age(), None);
    }
}
