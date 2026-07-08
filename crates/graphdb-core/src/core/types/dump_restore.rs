//! Dump/Restore type definitions

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpConfig {
    pub source_space: Option<String>,
    pub format: DumpFormat,
    pub include_schema: bool,
    pub include_data: bool,
    pub compression: CompressionType,
    pub output_path: PathBuf,
}

impl Default for DumpConfig {
    fn default() -> Self {
        Self {
            source_space: None,
            format: DumpFormat::Binary,
            include_schema: true,
            include_data: true,
            compression: CompressionType::Zstd,
            output_path: PathBuf::from("dump"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DumpFormat {
    Binary,
    JsonLines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpMetadata {
    pub version: String,
    pub timestamp: i64,
    pub spaces: Vec<SpaceDumpInfo>,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceDumpInfo {
    pub name: String,
    pub vertex_count: u64,
    pub edge_count: u64,
    pub tags: Vec<String>,
    pub edge_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RestoreConfig {
    pub source_path: PathBuf,
    pub target_space: Option<String>,
    pub overwrite_existing: bool,
    pub strict_mode: bool,
    pub restore_schema: bool,
    pub restore_data: bool,
}

impl Default for RestoreConfig {
    fn default() -> Self {
        Self {
            source_path: PathBuf::from("dump"),
            target_space: None,
            overwrite_existing: false,
            strict_mode: false,
            restore_schema: true,
            restore_data: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RestoreStats {
    pub spaces_restored: usize,
    pub vertices_restored: u64,
    pub edges_restored: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

impl RestoreStats {
    pub fn format_summary(&self) -> String {
        let mut output = String::new();
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Restore Statistics\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&format!("Spaces restored: {}\n", self.spaces_restored));
        output.push_str(&format!("Vertices restored: {}\n", self.vertices_restored));
        output.push_str(&format!("Edges restored: {}\n", self.edges_restored));
        output.push_str(&format!("Duration:        {:.3} s\n", self.duration_ms as f64 / 1000.0));
        if !self.errors.is_empty() {
            output.push_str(&format!("Errors:          {}\n", self.errors.len()));
            for err in self.errors.iter().take(5) {
                output.push_str(&format!("  - {}\n", err));
            }
        }
        output
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DumpError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("Unsupported dump version: {0}")]
    UnsupportedVersion(String),
    #[error("Dump file corrupted: {0}")]
    Corrupted(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Schema conflict: {0}")]
    SchemaConflict(String),
    #[error("Dump corrupted: {0}")]
    Corrupted(String),
    #[error("Space already exists: {0}")]
    SpaceExists(String),
    #[error("Deserialization error: {0}")]
    Deserialization(#[from] serde_json::Error),
}
