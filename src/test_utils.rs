#![allow(dead_code)]

pub mod assertions;
pub mod data_fixtures;
pub mod debug_helpers;
#[cfg(feature = "fulltext")]
pub mod fulltext_helpers;
pub mod query_helpers;
pub mod storage_helpers;
#[cfg(feature = "fulltext")]
pub mod sync_helpers;
pub mod test_scenario;
pub mod transaction_helpers;
pub mod validation_helpers;

#[cfg(feature = "embedded")]
pub mod c_api_helpers;

use crate::core::error::DBError;
use crate::core::metadata::SchemaManager;
use crate::storage::PropertyGraphConfig;
use crate::storage::{GraphStorage, StorageSchemaContextOps};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

pub type TestResult<T> = Result<T, Box<DBError>>;

pub struct TestStorage {
    storage: Arc<RwLock<GraphStorage>>,
    temp_path: Option<PathBuf>,
}

impl TestStorage {
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

    pub fn storage(&self) -> Arc<RwLock<GraphStorage>> {
        self.storage.clone()
    }

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
