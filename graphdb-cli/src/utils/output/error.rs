//! Output module error types

use std::fmt;
use std::io;

/// Errors that can occur during output operations
#[derive(Debug)]
pub enum OutputError {
    /// IO error from underlying writer
    Io(io::Error),
    /// JSON serialization/deserialization error
    Json(serde_json::Error),
    /// Invalid configuration
    InvalidConfig(String),
    /// Formatter not available for format
    FormatterNotAvailable(String),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputError::Io(e) => write!(f, "IO error: {}", e),
            OutputError::Json(e) => write!(f, "JSON error: {}", e),
            OutputError::InvalidConfig(msg) => write!(f, "Invalid config: {}", msg),
            OutputError::FormatterNotAvailable(fmt) => {
                write!(f, "Formatter not available for format: {}", fmt)
            }
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OutputError::Io(e) => Some(e),
            OutputError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for OutputError {
    fn from(e: io::Error) -> Self {
        OutputError::Io(e)
    }
}

impl From<serde_json::Error> for OutputError {
    fn from(e: serde_json::Error) -> Self {
        OutputError::Json(e)
    }
}

/// Result type alias for output operations
pub type Result<T> = std::result::Result<T, OutputError>;
