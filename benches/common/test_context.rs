// benches/common/test_context.rs
//! Test context and setup utilities

use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Benchmark context for managing test environment
pub struct BenchmarkContext {
    _temp_dir: TempDir,
    data_dir: PathBuf,
}

impl BenchmarkContext {
    /// Create new benchmark context
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().to_path_buf();

        Ok(Self {
            _temp_dir: temp_dir,
            data_dir,
        })
    }

    /// Get data directory path
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Generate a data file in the benchmark context
    pub fn generate_data(&self, filename: &str, content: &str) -> std::io::Result<PathBuf> {
        let path = self.data_dir.join(filename);
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

impl Default for BenchmarkContext {
    fn default() -> Self {
        Self::new().expect("Failed to create benchmark context")
    }
}
