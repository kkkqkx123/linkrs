//! Base definitions for streaming execution
//!
//! Contains ExecutionMode and related configuration for the streaming executor.

/// Execution mode selector
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Materialized execution (current system, push-based)
    Materialized,
    /// Streaming execution (new system, pull-based)
    Streaming,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Materialized
    }
}

impl ExecutionMode {
    pub fn is_materialized(&self) -> bool {
        *self == ExecutionMode::Materialized
    }

    pub fn is_streaming(&self) -> bool {
        *self == ExecutionMode::Streaming
    }
}

/// Configuration for streaming execution
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Chunk size (number of rows per chunk)
    pub chunk_size: usize,
    /// Maximum buffered chunks
    pub max_buffered_chunks: usize,
    /// Default execution mode
    pub default_mode: ExecutionMode,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            chunk_size: 1024,
            max_buffered_chunks: 10,
            default_mode: ExecutionMode::Materialized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_mode_default() {
        let mode = ExecutionMode::default();
        assert_eq!(mode, ExecutionMode::Materialized);
    }

    #[test]
    fn test_execution_mode_checks() {
        let materialized = ExecutionMode::Materialized;
        assert!(materialized.is_materialized());
        assert!(!materialized.is_streaming());

        let streaming = ExecutionMode::Streaming;
        assert!(!streaming.is_materialized());
        assert!(streaming.is_streaming());
    }

    #[test]
    fn test_streaming_config_default() {
        let config = StreamingConfig::default();
        assert_eq!(config.chunk_size, 1024);
        assert_eq!(config.max_buffered_chunks, 10);
        assert_eq!(config.default_mode, ExecutionMode::Materialized);
    }
}
