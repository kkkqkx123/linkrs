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
use graphdb_query::storage::PropertyGraphConfig;
use graphdb_query::storage::{GraphStorage, StorageSchemaContextOps};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

/// Lightweight result type for test code
pub type TestResult<T> = Result<T, Box<DBError>>;

/// Test Storage Instance Wrapper
///
/// Creates an in-memory storage instance with minimal resource footprint.
/// For tests requiring persistence, use `new_with_path()` instead.
pub struct TestStorage {
    storage: Arc<RwLock<GraphStorage>>,
    temp_path: Option<PathBuf>,
}

impl TestStorage {
    /// Creating a New Test Storage Instance (in-memory, minimal resource usage)
    pub fn new() -> TestResult<Self> {
        let storage = Arc::new(RwLock::new(
            GraphStorage::new_with_config(PropertyGraphConfig::test())
                .map_err(|e| Box::new(DBError::from(e)))?,
        ));
        Ok(Self {
            storage,
            temp_path: None,
        })
    }

    /// Creating a Test Storage Instance with a specific path and persistence
    pub fn new_with_path(path: PathBuf) -> TestResult<Self> {
        let storage = Arc::new(RwLock::new(
            GraphStorage::new_with_config(PropertyGraphConfig::test())
                .or_else(|_| GraphStorage::new_with_path(path.clone()))
                .map_err(|e| Box::new(DBError::from(e)))?,
        ));
        Ok(Self {
            storage,
            temp_path: Some(path),
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
        if let Some(path) = &self.temp_path {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
