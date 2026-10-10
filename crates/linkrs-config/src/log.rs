//! Log configuration

use serde::{Deserialize, Serialize};

/// Log configuration
///
/// `[log]` owns the main log stream: destination directory, file basename,
/// rotation policy and output format. Slow-query and audit logs share
/// `dir` and the megabyte rotation unit.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// Log level
    pub level: String,
    /// Log directory (absolute after loading; supports `~`)
    pub dir: String,
    /// Log file basename without extension. The `.log` extension and the
    /// rotation numbering are appended by the log library.
    pub basename: String,
    /// Maximum size of a single log file, in megabytes
    pub max_file_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: usize,
    /// Mirror log records to stdout in addition to the file
    pub stdout: bool,
    /// Emit one JSON object per line instead of the plain-text format
    pub json_format: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            dir: "logs".to_string(),
            basename: "linkrs".to_string(),
            max_file_size_mb: 100,
            max_files: 5,
            stdout: false,
            json_format: false,
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

        if self.basename.is_empty() {
            return Err("Log basename cannot be empty".to_string());
        }

        if self.max_file_size_mb == 0 {
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
        assert_eq!(config.basename, "linkrs");
        assert_eq!(config.max_file_size_mb, 100);
        assert_eq!(config.max_files, 5);
        assert!(!config.stdout);
        assert!(!config.json_format);
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

        let invalid_basename = LogConfig {
            basename: String::new(),
            ..Default::default()
        };
        assert!(invalid_basename.validate().is_err());

        let invalid_size = LogConfig {
            max_file_size_mb: 0,
            ..Default::default()
        };
        assert!(invalid_size.validate().is_err());
    }
}
