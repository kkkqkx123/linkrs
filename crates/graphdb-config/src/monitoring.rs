//! Monitoring and slow query log configuration

use serde::{Deserialize, Serialize};

/// Monitoring configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MonitoringConfig {
    /// Whether to enable monitoring
    pub enabled: bool,
    /// Memory cache size (retains the most recent N queries)
    pub memory_cache_size: usize,
    /// Slow query threshold (milliseconds)
    pub slow_query_threshold_ms: u64,
    /// Slow query log configuration
    #[serde(default)]
    pub slow_query_log: SlowQueryLogConfig,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_cache_size: 1000,
            slow_query_threshold_ms: 1000,
            slow_query_log: SlowQueryLogConfig::default(),
        }
    }
}

impl MonitoringConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.memory_cache_size == 0 {
            return Err("Memory cache size must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Slow query log configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SlowQueryLogConfig {
    /// Whether to enable slow query logging
    pub enabled: bool,
    /// Slow query threshold in milliseconds
    pub threshold_ms: u64,
    /// Log file path
    pub log_file_path: String,
    /// Maximum file size in MB before rotation
    pub max_file_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: u32,
    /// Whether to use verbose format
    pub verbose_format: bool,
    /// Async write buffer size
    pub buffer_size: usize,
    /// Whether to use JSON format
    pub json_format: bool,
}

impl Default for SlowQueryLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_ms: 1000,
            log_file_path: "logs/slow_query.log".to_string(),
            max_file_size_mb: 100,
            max_files: 5,
            verbose_format: false,
            buffer_size: 100,
            json_format: false,
        }
    }
}

impl SlowQueryLogConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.log_file_path.is_empty() {
            return Err("Slow query log file path cannot be empty".to_string());
        }

        if self.threshold_ms == 0 {
            return Err("Slow query threshold must be greater than 0".to_string());
        }

        if self.max_file_size_mb == 0 {
            return Err("Max file size must be greater than 0".to_string());
        }

        if self.max_files == 0 {
            return Err("Max files must be greater than 0".to_string());
        }

        if self.buffer_size == 0 {
            return Err("Buffer size must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Convert to SlowQueryConfig
    pub fn to_slow_query_config(&self) -> crate::core::stats::SlowQueryConfig {
        crate::core::stats::SlowQueryConfig {
            enabled: self.enabled,
            threshold_ms: self.threshold_ms,
            log_file_path: self.log_file_path.clone(),
            max_file_size_mb: self.max_file_size_mb,
            max_files: self.max_files,
            verbose_format: self.verbose_format,
            buffer_size: self.buffer_size,
            json_format: self.json_format,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitoring_config_default() {
        let config = MonitoringConfig::default();
        assert!(config.enabled);
        assert_eq!(config.memory_cache_size, 1000);
        assert_eq!(config.slow_query_threshold_ms, 1000);
    }

    #[test]
    fn test_slow_query_log_config_default() {
        let config = SlowQueryLogConfig::default();
        assert!(config.enabled);
        assert_eq!(config.threshold_ms, 1000);
        assert_eq!(config.log_file_path, "logs/slow_query.log");
        assert_eq!(config.max_file_size_mb, 100);
        assert_eq!(config.max_files, 5);
    }

    #[test]
    fn test_monitoring_config_validate() {
        let config = MonitoringConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = MonitoringConfig {
            memory_cache_size: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_slow_query_log_config_validate() {
        let config = SlowQueryLogConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = SlowQueryLogConfig {
            log_file_path: String::new(),
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
