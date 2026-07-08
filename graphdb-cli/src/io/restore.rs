//! CLI restore configuration

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CliRestoreConfig {
    pub source_path: PathBuf,
    pub database: String,
    pub overwrite: bool,
    pub strict: bool,
    pub schema_only: bool,
    pub data_only: bool,
}

impl Default for CliRestoreConfig {
    fn default() -> Self {
        Self {
            source_path: PathBuf::from("dump"),
            database: String::new(),
            overwrite: false,
            strict: false,
            schema_only: false,
            data_only: false,
        }
    }
}
