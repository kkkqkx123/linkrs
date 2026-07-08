//! Log configuration

use serde::{Deserialize, Serialize};

/// Log configuration
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogConfig {
    /// Log level
    pub level: String,
    /// Log directory
    pub dir: String,
    /// Log file name
    pub file: String,
    /// Maximum size of a single log file (bytes)
    pub max_file_size: u64,
    /// Maximum number of log files
    pub max_files: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            dir: "logs".to_string(),
            file: "graphdb".to_string(),
            max_file_size: 100 * 1024 * 1024, // 100MB
            max_files: 5,
        }
    }
}

impl LogConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.level.is_empty() {
            return Err("Log level cannot be empty".to_string());
        }

        if self.dir.is_empty() {
            return Err("Log directory cannot be empty".to_string());
        }

        if self.file.is_empty() {
            return Err("Log file name cannot be empty".to_string());
        }

        if self.max_file_size == 0 {
            return Err("Max file size must be greater than 0".to_string());
        }

        if self.max_files == 0 {
            return Err("Max files must be greater than 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.dir, "logs");
        assert_eq!(config.file, "graphdb");
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(config.max_files, 5);
    }

    #[test]
    fn test_log_config_validate() {
        let config = LogConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = LogConfig {
            level: String::new(),
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());
    }
}
