//! Database main module
//!
//! Provide the GraphDatabase structure as the main entry point for the embedded API.

use crate::core::{CoreError, CoreResult, QueryApi, SchemaApi, SpaceConfig};
use crate::embedded::config::{DatabaseConfig, EmbeddedVectorEngine};
use crate::embedded::result::QueryResult;
use crate::embedded::session::{GraphDatabaseInner, Session};
use crate::core::{StatsManager, Value};
use crate::search::FulltextConfig;
#[cfg(feature = "fulltext-search")]
use crate::search::FulltextIndexManager;
#[cfg(feature = "fulltext-search")]
use crate::search::SyncFailurePolicy;
use crate::storage::{GraphStorage, StorageClient};
#[cfg(feature = "vector")]
use crate::sync::backend::VectorBackend;
#[cfg(feature = "fulltext-search")]
use crate::sync::SyncConfig;
use crate::sync::SyncManager;
use crate::transaction::wal::SyncPolicy;
use crate::transaction::{TransactionManager, TransactionManagerConfig};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "vector-qdrant")]
use vector_client::{VectorClientConfig, VectorManager};

#[cfg(test)]
use crate::storage::MockStorage;

/// Create a VectorManager from the default configuration.
///
/// Uses the provided runtime handle to block on async VectorManager initialization.
#[cfg(feature = "vector-qdrant")]
fn create_vector_manager(
    vector_config: &VectorClientConfig,
    runtime: &tokio::runtime::Handle,
) -> CoreResult<Arc<VectorManager>> {
    let vector_manager = Arc::new(
        runtime
            .block_on(VectorManager::new(vector_config.clone()))
            .map_err(|e| {
                CoreError::Internal(format!("Failed to initialize vector manager: {}", e))
            })?,
    );
    Ok(vector_manager)
}

/// Derive the default local vector data directory from the database file path
/// (`<db_file>_vector` next to the database).
#[cfg(feature = "vector")]
fn default_local_vector_dir(db_path: &Path) -> std::path::PathBuf {
    let mut name = db_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push("_vector");
    db_path.with_file_name(name)
}

/// Build the vector backend from the embedded database configuration.
///
/// Returns `Ok(None)` only for explicit disablement (master switch off, a
/// disabled Qdrant client config, or an in-memory database without an explicit
/// local directory); engine construction failures are hard errors.
#[cfg(feature = "vector")]
fn create_vector_backend(
    config: &DatabaseConfig,
    runtime: &tokio::runtime::Handle,
) -> CoreResult<Option<VectorBackend>> {
    use std::path::PathBuf;

    if !config.vector.enabled {
        return Ok(None);
    }
    match &config.vector.engine {
        EmbeddedVectorEngine::Local => {
            let data_dir: PathBuf = match (&config.vector.local_data_dir, config.path()) {
                (Some(dir), _) => dir.clone(),
                (None, Some(path)) => default_local_vector_dir(path),
                // In-memory databases have no on-disk home; vectors stay off
                // unless an explicit directory is configured.
                (None, None) => return Ok(None),
            };
            let engine = vector_search::LocalVectorEngine::open(&data_dir).map_err(|e| {
                CoreError::Internal(format!("Failed to initialize local vector engine: {}", e))
            })?;
            Ok(Some(VectorBackend::local(engine)))
        }
        #[cfg(feature = "vector-qdrant")]
        EmbeddedVectorEngine::Qdrant(client_config) => {
            if !client_config.enabled {
                return Ok(None);
            }
            let manager = create_vector_manager(client_config, runtime)?;
            Ok(Some(VectorBackend::qdrant(manager)))
        }
    }
}

/// Attach a vector sync coordinator to an existing SyncManager (no-op if vector is disabled).
#[cfg(feature = "vector")]
fn attach_vector_coordinator(
    mut sync: SyncManager,
    config: &DatabaseConfig,
    runtime: &tokio::runtime::Handle,
) -> CoreResult<SyncManager> {
    if let Some(backend) = create_vector_backend(config, runtime)? {
        let vector_coordinator = Arc::new(
            crate::sync::vector_sync::VectorSyncCoordinator::new_without_embedding(
                backend,
                runtime.clone(),
            ),
        );
        sync = sync.with_vector_coordinator(vector_coordinator);
    }
    Ok(sync)
}

#[cfg(feature = "fulltext-search")]
type InitManagers = (Option<Arc<FulltextIndexManager>>, Option<Arc<SyncManager>>);
#[cfg(not(feature = "fulltext-search"))]
type InitManagers = (Option<Arc<()>>, Option<Arc<SyncManager>>);

/// Full init path when vector is enabled but fulltext is not: create a sync manager
/// that only hosts the vector coordinator.
#[cfg(all(feature = "vector", not(feature = "fulltext-search")))]
fn setup_sync_with_vector_only(
    config: &DatabaseConfig,
    runtime: &tokio::runtime::Handle,
) -> CoreResult<InitManagers> {
    let Some(backend) = create_vector_backend(config, runtime)? else {
        return Ok((None, None));
    };
    let vector_coordinator = Arc::new(
        crate::sync::vector_sync::VectorSyncCoordinator::new_without_embedding(
            backend,
            runtime.clone(),
        ),
    );

    let mut sync = SyncManager::new_without_fulltext();
    sync = sync.with_vector_coordinator(vector_coordinator);
    Ok((None, Some(Arc::new(sync))))
}

/// Full init path when both vector and fulltext are enabled.
#[cfg(all(feature = "vector", feature = "fulltext-search"))]
fn setup_sync_with_vector_only(
    config: &DatabaseConfig,
    runtime: &tokio::runtime::Handle,
) -> CoreResult<InitManagers> {
    let Some(backend) = create_vector_backend(config, runtime)? else {
        return Ok((None, None));
    };
    let vector_coordinator = Arc::new(
        crate::sync::vector_sync::VectorSyncCoordinator::new_without_embedding(
            backend,
            runtime.clone(),
        ),
    );

    let sync_config = SyncConfig::default();
    let batch_config = crate::sync::batch::BatchConfig::from(sync_config.clone());
    let manager = Arc::new(
        FulltextIndexManager::new(FulltextConfig::default()).map_err(|e| {
            CoreError::Internal(format!("Failed to initialize fulltext manager: {}", e))
        })?,
    );
    let sync_coordinator = Arc::new(crate::sync::coordinator::SyncCoordinator::new(
        manager.clone(),
        batch_config,
    ));
    let mut sync = SyncManager::with_sync_config(sync_coordinator, sync_config);
    sync = sync.with_vector_coordinator(vector_coordinator);
    Ok((None, Some(Arc::new(sync))))
}

/// Embedded GraphDB database
///
/// This is the main entry point for the embedded API, offering a simple way of use similar to that of SQLite.
/// The sqlite3 structure corresponding to SQLite.
///
/// # Example
///
/// ```rust
/// use graphdb_api::embedded::{GraphDatabase, DatabaseConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
// Open the database
/// let db = GraphDatabase::open("my_database")?;
///
// Create a session
/// let mut session = db.session()?;
///
// Switch to the image space
/// session.use_space("test_space")?;
///
// Execute the query
/// let result = session.execute("MATCH (n) RETURN n")?;
///
// Using a transaction
/// let txn = session.begin_transaction()?;
/// txn.execute("CREATE TAG user(name string)")?;
/// txn.commit()?;
///
// The database is automatically closed when the `db` variable goes out of scope.
///
/// # Ok(())
/// # }
/// ```
pub struct GraphDatabase<S: StorageClient + Clone + 'static> {
    inner: Arc<GraphDatabaseInner<S>>,
    config: DatabaseConfig,
}

impl GraphDatabase<GraphStorage> {
    /// Open or create a database (in file mode).
    ///
    /// # Parameters
    /// `path` – The path to the database file
    ///
    /// # Back
    /// Return the GraphDatabase instance upon successful completion.
    /// - Return error on failure
    pub fn open(path: impl AsRef<Path>) -> CoreResult<Self> {
        let config = DatabaseConfig::file(path);
        Self::open_with_config(config)
    }

    /// Create a memory database
    ///
    /// # Back
    /// - Returns the GraphDatabase instance on success
    /// - Return error on failure
    pub fn open_in_memory() -> CoreResult<Self> {
        let config = DatabaseConfig::memory();
        Self::open_with_config(config)
    }

    /// Open the database using the configuration settings.
    ///
    /// # Parameters
    /// `config` – Database configuration
    ///
    /// # Return
    /// - Returns GraphDatabase instance on success
    /// - Return error on failure
    pub fn open_with_config(config: DatabaseConfig) -> CoreResult<Self> {
        // Create a dedicated tokio runtime for vector operations in embedded mode.
        // This runtime lives for the lifetime of the GraphDatabase and is stored
        // to prevent it from being dropped.
        #[cfg(feature = "vector")]
        let vector_runtime =
            Arc::new(tokio::runtime::Runtime::new().map_err(|e| {
                CoreError::Internal(format!("Failed to create tokio runtime: {}", e))
            })?);

        let (storage, _enable_wal, _sync_policy) = if config.is_memory() {
            let storage = GraphStorage::new().map_err(|e| {
                CoreError::StorageError(format!("Failed to initialize memory store: {}", e))
            })?;
            (storage, true, Some(SyncPolicy::EveryWrite))
        } else {
            let path = config
                .path()
                .ok_or_else(|| CoreError::StorageError("Database path is empty".to_string()))?;
            let enable_wal = config.enable_wal;
            let sync_policy = sync_mode_to_policy(config.sync_mode);
            let storage =
                GraphStorage::open_with_persistence(path.to_path_buf(), enable_wal, sync_policy)
                    .map_err(|e| {
                        CoreError::StorageError(format!("Failed to initialize storage: {}", e))
                    })?;
            (storage, enable_wal, sync_policy)
        };

        let version_manager = storage.version_manager();
        let storage = Arc::new(RwLock::new(storage));

        let fulltext_config = FulltextConfig::default();

        #[cfg_attr(not(feature = "fulltext-search"), allow(unused_variables))]
        let (fulltext_manager, mut sync_manager): InitManagers = if fulltext_config.enabled {
            #[cfg(feature = "fulltext-search")]
            {
                let manager: Arc<FulltextIndexManager> = Arc::new(
                    FulltextIndexManager::new(fulltext_config.clone())
                        .map_err(|e| CoreError::Internal(e.to_string()))?,
                );

                let sync_config = SyncConfig {
                    queue_size: fulltext_config.sync.queue_size,
                    commit_interval_ms: fulltext_config.sync.commit_interval_ms,
                    batch_size: fulltext_config.sync.batch_size,
                    failure_policy: SyncFailurePolicy::FailOpen,
                };

                let batch_config = crate::sync::batch::BatchConfig::from(sync_config.clone());
                let sync_coordinator = Arc::new(crate::sync::coordinator::SyncCoordinator::new(
                    manager.clone(),
                    batch_config,
                ));

                let sync = SyncManager::with_sync_config(sync_coordinator.clone(), sync_config);

                #[cfg(feature = "vector")]
                let sync = attach_vector_coordinator(sync, &config, vector_runtime.handle())?;

                let sync = Arc::new(sync);
                (Some(manager), Some(sync))
            }
            #[cfg(not(feature = "fulltext-search"))]
            {
                #[cfg(feature = "vector")]
                {
                    setup_sync_with_vector_only(&config, vector_runtime.handle())?
                }
                #[cfg(not(feature = "vector"))]
                {
                    (None, None)
                }
            }
        } else {
            #[cfg(feature = "vector")]
            {
                setup_sync_with_vector_only(&config, vector_runtime.handle())?
            }
            #[cfg(not(feature = "vector"))]
            {
                (None, None)
            }
        };

        if let (Some(path), Some(manager)) = (
            config.path(),
            sync_manager
                .as_mut()
                .and_then(|m| Arc::<SyncManager>::get_mut(m)),
        ) {
            manager
                .configure_outbox(path.join("outbox/outbox.sqlite"))
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            let _ = manager.retry_outbox_sync();
        }

        let txn_manager_config = TransactionManagerConfig::default();

        // Create shared StatsManager for all components (before TransactionManager to enable wiring)
        let stats_manager = Arc::new(StatsManager::new());
        if let Some(manager) = sync_manager.as_mut().and_then(Arc::get_mut) {
            manager.set_stats_manager(stats_manager.clone());
        }

        let mut txn_manager = TransactionManager::with_shared_version_manager(
            txn_manager_config,
            stats_manager.clone(),
            version_manager,
        );
        if let Some(ref sync) = sync_manager {
            txn_manager = txn_manager.with_sync_manager(sync.clone());
        }
        let txn_manager = Arc::new(txn_manager);

        let query_api = if let Some(ref sync) = sync_manager {
            Arc::new(RwLock::new(QueryApi::with_sync_manager(
                storage.clone(),
                stats_manager.clone(),
                sync.clone(),
            )))
        } else {
            Arc::new(RwLock::new(QueryApi::new(
                storage.clone(),
                stats_manager.clone(),
            )))
        };
        let schema_api = SchemaApi::new(storage.clone());

        let inner = Arc::new(GraphDatabaseInner {
            query_api,
            schema_api,
            txn_manager,
            storage,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager,
            sync_manager,
            stats_manager,
            #[cfg(feature = "vector")]
            vector_runtime,
        });

        Ok(Self { inner, config })
    }
}

impl<S: StorageClient + Clone + 'static> GraphDatabase<S> {
    /// Create a new session.
    ///
    /// # Return
    /// Return the Session instance upon successful completion.
    /// - Return error on failure
    pub fn session(&self) -> CoreResult<Session<S>> {
        Ok(Session::new(self.inner.clone()))
    }

    /// Perform simple queries (a convenient method)
    ///
    /// This method creates a temporary session to execute the query, which is suitable for simple, one-time query scenarios.
    /// For complex scenarios, it is recommended to use session() to create a session.
    ///
    /// # Parameters
    /// `query` – A string representing the query statement.
    ///
    /// # Return
    /// Return the query results when successful.
    /// - Return error on failure
    pub fn execute(&self, query: &str) -> CoreResult<QueryResult> {
        let session = self.session()?;
        session.execute(query)
    }

    /// Executing parameterized queries (a convenient method)
    ///
    /// # Parameters
    /// - `query` - query statement string
    /// - `params` – Query parameters
    ///
    /// # Return
    /// - Returns query results on success
    /// - Return error on failure
    pub fn execute_with_params(
        &self,
        query: &str,
        params: HashMap<String, Value>,
    ) -> CoreResult<QueryResult> {
        let session = self.session()?;
        session.execute_with_params(query, params)
    }

    /// Creating a graphical space (an easy method)
    ///
    /// # Parameters
    /// - `name' - space name
    /// `config` – Space configuration
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn create_space(&self, name: &str, space_config: SpaceConfig) -> CoreResult<()> {
        let session = self.session()?;
        session.create_space(name, space_config)
    }

    /// Deletion of map space (convenient method)
    ///
    /// # Parameters
    /// - `name' - space name
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn drop_space(&self, name: &str) -> CoreResult<()> {
        let session = self.session()?;
        session.drop_space(name)
    }

    /// List all graph spaces (convenience method)
    pub fn list_spaces(&self) -> CoreResult<Vec<String>> {
        let session = self.session()?;
        session.list_spaces()
    }

    /// Get Configuration
    pub fn config(&self) -> &DatabaseConfig {
        &self.config
    }

    /// Checking for in-memory databases
    pub fn is_memory(&self) -> bool {
        self.config.is_memory()
    }

    /// Getting a reference to the storage client
    ///
    /// # Return
    /// - RwLockReadGuard for Storage Clients
    pub fn storage(&self) -> parking_lot::RwLockReadGuard<'_, S> {
        self.inner.storage.read()
    }

    /// Getting a mutable reference to the storage client
    ///
    /// # Return
    /// - RwLockWriteGuard for Storage Clients
    pub fn storage_mut(&self) -> parking_lot::RwLockWriteGuard<'_, S> {
        self.inner.storage.write()
    }
}

// To support Send + Sync
// Safety Notes:
// 1. GraphDatabase uses Arc<GraphDatabaseInner<S>> to share data internally, Arc itself is Send + Sync.
// 2. QueryApi in GraphDatabaseInner is Mutex-protected for thread-safety.
// 3. StorageClient is required to implement Clone + 'static to ensure safe cross-thread delivery.
// 4. TransactionManager uses Arc wrappers, which can be safely shared across threads.
// 5. config is a standalone DatabaseConfig, safe to pass across threads.
// GraphDatabase can therefore securely implement Send and Sync.
unsafe impl<S: StorageClient + Clone + 'static> Send for GraphDatabase<S> {}
unsafe impl<S: StorageClient + Clone + 'static> Sync for GraphDatabase<S> {}

fn sync_mode_to_policy(mode: crate::embedded::config::SyncMode) -> Option<SyncPolicy> {
    match mode {
        crate::embedded::config::SyncMode::Full => Some(SyncPolicy::EveryWrite),
        crate::embedded::config::SyncMode::Normal => Some(SyncPolicy::EveryWrite),
        crate::embedded::config::SyncMode::Off => Some(SyncPolicy::Never),
    }
}

#[cfg(test)]
impl GraphDatabase<MockStorage> {
    /// Create database for testing (using Mock storage)
    ///
    /// Note: This method is for testing only, should use `GraphDatabase::open()` in production
    #[cfg(test)]
    pub fn open_test() -> CoreResult<Self> {
        let storage = MockStorage::new().map_err(|e| {
            CoreError::StorageError(format!("Failed to initialize Mock store: {}", e))
        })?;

        let storage = Arc::new(RwLock::new(storage));

        let txn_manager_config = TransactionManagerConfig::default();
        let txn_manager = Arc::new(TransactionManager::new(txn_manager_config));

        let stats_manager = Arc::new(StatsManager::new());
        let query_api = Arc::new(RwLock::new(QueryApi::new(
            storage.clone(),
            stats_manager.clone(),
        )));
        let schema_api = SchemaApi::new(storage.clone());

        #[cfg(feature = "vector")]
        let vector_runtime =
            Arc::new(tokio::runtime::Runtime::new().expect("Failed to create tokio runtime"));

        let inner = Arc::new(GraphDatabaseInner {
            query_api,
            schema_api,
            txn_manager,
            storage,
            fulltext_manager: None,
            sync_manager: None,
            stats_manager,
            #[cfg(feature = "vector")]
            vector_runtime,
        });

        Ok(Self {
            inner,
            config: DatabaseConfig::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_config() {
        let config = DatabaseConfig::memory();
        assert!(config.is_memory());

        let config = DatabaseConfig::file("/tmp/test.db");
        assert!(!config.is_memory());
    }

    #[cfg(feature = "vector")]
    #[test]
    fn vector_backend_respects_explicit_disable() {
        let config = DatabaseConfig::file("/tmp/some.db").with_vector_enabled(false);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = create_vector_backend(&config, runtime.handle()).unwrap();
        assert!(backend.is_none(), "master switch off must disable vectors");
    }

    #[cfg(feature = "vector")]
    #[test]
    fn vector_backend_derives_local_dir_from_database_path() {
        let directory = tempfile::TempDir::new().unwrap();
        let db_path = directory.path().join("graph.db");
        let config = DatabaseConfig::file(&db_path);
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let backend = create_vector_backend(&config, runtime.handle())
            .expect("local engine construction must succeed");
        assert!(
            backend.as_local().is_some(),
            "file databases default to the local engine"
        );

        let expected_dir = directory.path().join("graph.db_vector");
        assert!(
            expected_dir.exists(),
            "the local engine data directory must be derived as <db_file>_vector"
        );
    }

    #[cfg(feature = "vector")]
    #[test]
    fn vector_backend_stays_off_for_memory_database_without_directory() {
        let config = DatabaseConfig::memory();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = create_vector_backend(&config, runtime.handle()).unwrap();
        assert!(
            backend.is_none(),
            "in-memory databases have no on-disk home for vectors unless configured explicitly"
        );
    }
}
