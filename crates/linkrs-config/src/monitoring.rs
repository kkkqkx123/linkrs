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
    /// Row interval for query-progress notifications. `0` disables progress
    /// emission (zero hot-path overhead). When greater than zero, the execution
    /// runtime reports processed-row watermarks to attached progress observers.
    #[serde(default)]
    pub progress_report_rows_interval: u64,
    /// Portrait cache size is `memory_cache_size`; this keeps the name for
    /// handler-facing snapshot limits.
    #[serde(default = "default_histogram_max_samples")]
    pub histogram_max_samples: usize,
    /// Timeseries ring retention in seconds.
    #[serde(default = "default_timeseries_retention_secs")]
    pub timeseries_retention_secs: usize,
    /// Resource sampling interval in seconds.
    #[serde(default = "default_resource_sample_interval_secs")]
    pub resource_sample_interval_secs: u64,
}

fn default_histogram_max_samples() -> usize {
    10000
}

fn default_timeseries_retention_secs() -> usize {
    3600
}

fn default_resource_sample_interval_secs() -> u64 {
    60
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_cache_size: 1000,
            slow_query_threshold_ms: 1000,
            slow_query_log: SlowQueryLogConfig::default(),
            progress_report_rows_interval: 0,
            histogram_max_samples: default_histogram_max_samples(),
            timeseries_retention_secs: default_timeseries_retention_secs(),
            resource_sample_interval_secs: default_resource_sample_interval_secs(),
        }
    }
}

impl MonitoringConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.memory_cache_size == 0 {
            return Err("Memory cache size must be greater than 0".to_string());
        }
        if self.histogram_max_samples < 100 || self.histogram_max_samples > 100_000 {
            return Err("Histogram max samples must be within 100..=100000".to_string());
        }
        if self.timeseries_retention_secs < 60 || self.timeseries_retention_secs > 86400 {
            return Err("Timeseries retention must be within 60..=86400 seconds".to_string());
        }
        if self.resource_sample_interval_secs == 0 || self.resource_sample_interval_secs > 3600 {
            return Err("Resource sample interval must be within 1..=3600 seconds".to_string());
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
    pub fn to_slow_query_config(&self) -> linkrs_metrics::SlowQueryConfig {
        linkrs_metrics::SlowQueryConfig {
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

        let invalid_histogram = MonitoringConfig {
            histogram_max_samples: 10,
            ..Default::default()
        };
        assert!(invalid_histogram.validate().is_err());

        let invalid_retention = MonitoringConfig {
            timeseries_retention_secs: 10,
            ..Default::default()
        };
        assert!(invalid_retention.validate().is_err());
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
