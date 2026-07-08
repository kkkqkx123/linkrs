//! WAL writer factory

use super::dummy::DummyWalWriter;
use super::local::LocalWalWriter;
use crate::core::wal::traits::WalWriter;
use crate::core::wal::types::{WalError, WalResult};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Type alias for a factory function that creates WAL writers
pub type WalWriterFactoryFn =
    Arc<dyn Fn(&str, u32) -> WalResult<Box<dyn WalWriter>> + Send + Sync>;

/// Global registry of WAL writer factory functions
static WAL_WRITER_REGISTRY: Lazy<Arc<Mutex<HashMap<String, WalWriterFactoryFn>>>> =
    Lazy::new(|| {
        let mut registry = HashMap::new();

        // Register built-in writers
        registry.insert(
            "file".to_string(),
            Arc::new(|uri: &str, thread_id: u32| {
                let writer: Box<dyn WalWriter> = Box::new(LocalWalWriter::new(uri, thread_id));
                Ok(writer)
            }) as WalWriterFactoryFn,
        );

        registry.insert(
            "dummy".to_string(),
            Arc::new(|_uri: &str, _thread_id: u32| {
                let writer: Box<dyn WalWriter> = Box::new(DummyWalWriter::new());
                Ok(writer)
            }) as WalWriterFactoryFn,
        );

        Arc::new(Mutex::new(registry))
    });

/// WAL writer factory with extensible registry
pub struct WalWriterFactory;

impl WalWriterFactory {
    /// Create a WAL writer based on the URI scheme
    ///
    /// Supports both built-in writers ("file", "dummy") and custom registered writers.
    ///
    /// # Arguments
    ///
    /// * `wal_uri` - URI specifying the writer type (e.g., "file://path", "dummy", "custom://...")
    /// * `thread_id` - Thread identifier for the writer
    ///
    /// # Returns
    ///
    /// A Result containing a boxed WAL writer or an error
    pub fn create_wal_writer(wal_uri: &str, thread_id: u32) -> WalResult<Box<dyn WalWriter>> {
        let scheme = Self::get_scheme(wal_uri);
        let registry = WAL_WRITER_REGISTRY.lock().map_err(|_| {
            WalError::IoError("Failed to acquire writer registry lock".to_string())
        })?;

        if let Some(factory_fn) = registry.get(scheme.as_str()) {
            factory_fn(wal_uri, thread_id)
        } else {
            Err(WalError::IoError(format!(
                "Unknown WAL writer scheme: {} (registered: {})",
                scheme,
                registry.keys().cloned().collect::<Vec<_>>().join(", ")
            )))
        }
    }

    /// Create a dummy WAL writer
    pub fn create_dummy_wal_writer() -> Box<dyn WalWriter> {
        Box::new(DummyWalWriter::new())
    }

    /// Register a custom WAL writer factory
    ///
    /// This allows external code to register custom WAL writer implementations.
    ///
    /// # Arguments
    ///
    /// * `scheme` - The URI scheme to register (e.g., "s3", "redis", "custom")
    /// * `factory` - A factory function that creates writer instances
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure
    ///
    /// # Example
    ///
    /// ```ignore
    /// use graphdb::transaction::wal::writer::WalWriterFactory;
    ///
    /// WalWriterFactory::register_writer(
    ///     "s3".to_string(),
    ///     Arc::new(|uri, thread_id| {
    ///         Ok(Box::new(S3WalWriter::new(uri, thread_id)?))
    ///     })
    /// )?;
    /// ```
    pub fn register_writer(scheme: String, factory: WalWriterFactoryFn) -> WalResult<()> {
        let mut registry = WAL_WRITER_REGISTRY.lock().map_err(|_| {
            WalError::IoError("Failed to acquire writer registry lock".to_string())
        })?;

        if registry.contains_key(&scheme) {
            return Err(WalError::IoError(format!(
                "WAL writer scheme '{}' is already registered",
                scheme
            )));
        }

        registry.insert(scheme, factory);
        Ok(())
    }

    /// Override an existing WAL writer factory
    ///
    /// Unlike `register_writer`, this allows replacing an existing factory.
    ///
    /// # Arguments
    ///
    /// * `scheme` - The URI scheme to replace
    /// * `factory` - The new factory function
    ///
    /// # Returns
    ///
    /// A Result containing the old factory or an error
    pub fn override_writer(
        scheme: String,
        factory: WalWriterFactoryFn,
    ) -> WalResult<Option<WalWriterFactoryFn>> {
        let mut registry = WAL_WRITER_REGISTRY.lock().map_err(|_| {
            WalError::IoError("Failed to acquire writer registry lock".to_string())
        })?;

        Ok(registry.insert(scheme, factory))
    }

    /// List all registered WAL writer schemes
    pub fn list_schemes() -> WalResult<Vec<String>> {
        let registry = WAL_WRITER_REGISTRY.lock().map_err(|_| {
            WalError::IoError("Failed to acquire writer registry lock".to_string())
        })?;

        Ok(registry.keys().cloned().collect())
    }

    fn get_scheme(uri: &str) -> String {
        if let Some(pos) = uri.find("://") {
            uri[..pos].to_string()
        } else {
            "file".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_file_writer() {
        let result = WalWriterFactory::create_wal_writer("file:///tmp/wal", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_dummy_writer() {
        let result = WalWriterFactory::create_wal_writer("dummy://test", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_schemes() {
        let schemes = WalWriterFactory::list_schemes().expect("Failed to list schemes");
        assert!(schemes.contains(&"file".to_string()));
        assert!(schemes.contains(&"dummy".to_string()));
    }

    #[test]
    fn test_register_custom_writer() {
        let result = WalWriterFactory::register_writer(
            "test".to_string(),
            Arc::new(|_uri: &str, _thread_id: u32| {
                Ok(Box::new(DummyWalWriter::new()))
            }),
        );
        assert!(result.is_ok());

        // Try to register again - should fail
        let result = WalWriterFactory::register_writer(
            "test".to_string(),
            Arc::new(|_uri: &str, _thread_id: u32| {
                Ok(Box::new(DummyWalWriter::new()))
            }),
        );
        assert!(result.is_err());
    }
}
