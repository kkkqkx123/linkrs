//! Configuration for output module

/// Output mode - where to send output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    /// Output to console only (stdout/stderr)
    #[default]
    Console,
    /// Output to file only
    File,
    /// Output to both console and file
    Both,
}

impl std::str::FromStr for OutputMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "console" => Ok(OutputMode::Console),
            "file" => Ok(OutputMode::File),
            "both" => Ok(OutputMode::Both),
            _ => Err(format!("Invalid output mode: {}", s)),
        }
    }
}

impl std::fmt::Display for OutputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputMode::Console => write!(f, "console"),
            OutputMode::File => write!(f, "file"),
            OutputMode::Both => write!(f, "both"),
        }
    }
}

/// Configuration for output operations
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Output file path (required when mode is File or Both)
    pub file_path: Option<std::path::PathBuf>,
    /// Output mode
    pub mode: OutputMode,
    /// Whether to append to file (if false, overwrites)
    pub append: bool,
    /// Buffer size for file output (in bytes)
    pub buffer_size: usize,
}

impl OutputConfig {
    /// Create a new output configuration with console mode
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new output configuration for file output
    pub fn file(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            file_path: Some(path.into()),
            mode: OutputMode::File,
            ..Default::default()
        }
    }

    /// Create a new output configuration for both console and file output
    pub fn both(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            file_path: Some(path.into()),
            mode: OutputMode::Both,
            ..Default::default()
        }
    }

    /// Set the output file path
    pub fn with_file_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Set the output mode
    pub fn with_mode(mut self, mode: OutputMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set whether to append to file
    pub fn with_append(mut self, append: bool) -> Self {
        self.append = append;
        self
    }

    /// Set the buffer size
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        match self.mode {
            OutputMode::File | OutputMode::Both => {
                if self.file_path.is_none() {
                    return Err(
                        "File path is required when output mode is 'file' or 'both'".to_string()
                    );
                }
            }
            OutputMode::Console => {}
        }
        Ok(())
    }

    /// Check if file output is enabled
    pub fn has_file_output(&self) -> bool {
        matches!(self.mode, OutputMode::File | OutputMode::Both) && self.file_path.is_some()
    }

    /// Check if console output is enabled
    pub fn has_console_output(&self) -> bool {
        matches!(self.mode, OutputMode::Console | OutputMode::Both)
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            file_path: None,
            mode: OutputMode::Console,
            append: false,
            buffer_size: 8192, // 8KB default buffer
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_mode_from_str() {
        assert_eq!(
            "console".parse::<OutputMode>().unwrap(),
            OutputMode::Console
        );
        assert_eq!("file".parse::<OutputMode>().unwrap(), OutputMode::File);
        assert_eq!("both".parse::<OutputMode>().unwrap(), OutputMode::Both);
        assert!("invalid".parse::<OutputMode>().is_err());
    }

    #[test]
    fn test_output_mode_display() {
        assert_eq!(OutputMode::Console.to_string(), "console");
        assert_eq!(OutputMode::File.to_string(), "file");
        assert_eq!(OutputMode::Both.to_string(), "both");
    }

    #[test]
    fn test_output_config_default() {
        let config = OutputConfig::default();
        assert_eq!(config.mode, OutputMode::Console);
        assert!(config.file_path.is_none());
        assert!(!config.append);
        assert_eq!(config.buffer_size, 8192);
    }

    #[test]
    fn test_output_config_file() {
        let path = std::path::PathBuf::from("/tmp/test.log");
        let config = OutputConfig::file(&path);

        assert_eq!(config.mode, OutputMode::File);
        assert_eq!(config.file_path, Some(path));
    }

    #[test]
    fn test_output_config_validation() {
        let config = OutputConfig::default();
        assert!(config.validate().is_ok());

        let mut config = OutputConfig::default();
        config.mode = OutputMode::File;
        assert!(config.validate().is_err());

        let config = OutputConfig::file("/tmp/test.log");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_output_config_builder() {
        let config = OutputConfig::new()
            .with_file_path("/tmp/test.log")
            .with_mode(OutputMode::Both)
            .with_append(true)
            .with_buffer_size(16384);

        assert_eq!(config.mode, OutputMode::Both);
        assert!(config.file_path.is_some());
        assert!(config.append);
        assert_eq!(config.buffer_size, 16384);
    }
}
