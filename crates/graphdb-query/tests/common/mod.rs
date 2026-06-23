//! Integration Testing Shared Tool Module
//!
//! Provide test infrastructure and helper functions for all integration tests

#![allow(dead_code)]

pub mod assertions;
pub mod data_fixtures;
pub mod query_helpers;
pub mod storage_helpers;
pub mod test_scenario;

use graphdb_query::core::error::DBError;
use graphdb_query::core::metadata::SchemaManager;
use graphdb_query::storage::{GraphStorage, StorageSchemaContextOps};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

/// Lightweight result type for test code
pub type TestResult<T> = Result<T, Box<DBError>>;

/// Test Storage Instance Wrapper
///
/// Ensure that each test has a separate storage environment using a temporary folder in the project directory.
/// Automatic cleanup of temporary directories after testing
pub struct TestStorage {
    storage: Arc<RwLock<GraphStorage>>,
    temp_path: PathBuf,
}

impl TestStorage {
    /// Creating a New Test Storage Instance
    pub fn new() -> TestResult<Self> {
        let temp_dir = tempfile::tempdir().map_err(|e| DBError::io(e.to_string()))?;
        let db_path = temp_dir.path().join("test.db");

        let storage = Arc::new(RwLock::new(
            GraphStorage::new_with_path(db_path).map_err(|e| Box::new(DBError::from(e)))?,
        ));
        Ok(Self {
            storage,
            temp_path: temp_dir.path().to_path_buf(),
        })
    }

    /// Creating a Test Storage Instance with a specific path
    pub fn new_with_path(path: PathBuf) -> TestResult<Self> {
        let storage = Arc::new(RwLock::new(
            GraphStorage::new_with_path(path).map_err(|e| Box::new(DBError::from(e)))?,
        ));
        Ok(Self {
            storage,
            temp_path: PathBuf::new(),
        })
    }

    /// Getting a Reference to a Storage Instance
    pub fn storage(&self) -> Arc<RwLock<GraphStorage>> {
        self.storage.clone()
    }

    /// Getting the Schema Manager from Storage
    pub fn schema_manager(&self) -> Arc<SchemaManager> {
        let storage = self.storage.read();
        storage
            .get_schema_manager()
            .expect("Storage should provide a schema manager")
    }
}

impl Drop for TestStorage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp_path);
    }
}
