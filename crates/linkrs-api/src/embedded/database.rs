//! Database main module
//!
//! Provide the GraphDatabase structure as the main entry point for the embedded API.

use crate::api_core::{CoreError, CoreResult, QueryApi, SchemaApi, SpaceConfig};
use crate::embedded::config::DatabaseConfig;
#[cfg(feature = "vector")]
use crate::embedded::config::EmbeddedVectorEngine;
use crate::embedded::result::QueryResult;
use crate::embedded::session::{GraphDatabaseInner, Session};
use linkrs_core::Value;
use linkrs_fulltext::FulltextConfig;
#[cfg(feature = "fulltext")]
use linkrs_fulltext::FulltextIndexManager;
#[cfg(feature = "fulltext")]
use linkrs_fulltext::SyncFailurePolicy;
use linkrs_metrics::StatsManager;
use linkrs_storage::{GraphStorage, PersistenceConfig, StorageClient};
#[cfg(feature = "vector")]
use linkrs_sync::backend::VectorBackend;
#[cfg(feature = "fulltext")]
use linkrs_sync::SyncConfig;
use linkrs_sync::SyncManager;
use linkrs_transaction::wal::SyncPolicy;
use linkrs_transaction::{TransactionManager, TransactionManagerConfig};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "vector-qdrant")]
use vector_client::{VectorClientConfig, VectorManager};

#[cfg(test)]
use linkrs_storage::MockStorage;

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
    #[cfg_attr(not(feature = "vector-qdrant"), allow(unused_variables))]
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
            let engine = simvec::LocalVectorEngine::open(&data_dir).map_err(|e| {
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

/// Build the vector sync coordinator, or `None` when the vector
/// subsystem is disabled or has no usable backend.
#[cfg(feature = "vector")]
fn create_vector_coordinator(
    config: &DatabaseConfig,
    runtime: Option<&tokio::runtime::Handle>,
) -> CoreResult<Option<Arc<linkrs_sync::vector_sync::VectorSyncCoordinator>>> {
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    let Some(backend) = create_vector_backend(config, runtime)? else {
        return Ok(None);
    };
    Ok(Some(Arc::new(
        linkrs_sync::vector_sync::VectorSyncCoordinator::new_without_embedding(
            backend,
            runtime.clone(),
        ),
    )))
}

/// Secondary subsystem managers assembled when the database opens.
///
/// The fulltext index manager exists only when the fulltext feature is
/// compiled in and enabled by configuration; the sync manager exists only
/// when at least one coordinator (fulltext or vector) was attached. An
/// in-memory database without an explicit vector directory therefore ends
/// up with neither.
#[cfg(feature = "fulltext")]
struct SecondaryManagers {
    fulltext: Option<Arc<FulltextIndexManager>>,
    sync: Option<Arc<SyncManager>>,
}

#[cfg(not(feature = "fulltext"))]
struct SecondaryManagers {
    sync: Option<Arc<SyncManager>>,
}

/// Build the fulltext and vector managers for one open.
fn build_secondary_managers(
    #[cfg_attr(not(feature = "vector"), allow(unused_variables))] config: &DatabaseConfig,
    #[cfg_attr(not(feature = "fulltext"), allow(unused_variables))]
    fulltext_config: &FulltextConfig,
    #[cfg_attr(not(feature = "vector"), allow(unused_variables))] vector_runtime: Option<
        &tokio::runtime::Handle,
    >,
) -> CoreResult<SecondaryManagers> {
    #[cfg(feature = "fulltext")]
    if fulltext_config.enabled {
        let manager: Arc<FulltextIndexManager> = Arc::new(
            FulltextIndexManager::new(fulltext_config.clone())
                .map_err(|e| CoreError::FulltextError(e.to_string()))?,
        );
        let sync_config = SyncConfig {
            queue_size: fulltext_config.sync.queue_size,
            commit_interval_ms: fulltext_config.sync.commit_interval_ms,
            batch_size: fulltext_config.sync.batch_size,
            failure_policy: SyncFailurePolicy::FailOpen,
        };
        let batch_config = linkrs_sync::batch::BatchConfig::from(sync_config.clone());
        let sync_coordinator = Arc::new(linkrs_sync::fulltext_sync::FulltextSyncCoordinator::new(
            manager.clone(),
            batch_config,
        ));
        #[cfg_attr(not(feature = "vector"), allow(unused_mut))]
        let mut sync = SyncManager::with_sync_config(sync_coordinator, sync_config);
        #[cfg(feature = "vector")]
        if let Some(coordinator) = create_vector_coordinator(config, vector_runtime)? {
            sync = sync.with_vector_coordinator(coordinator);
        }
        return Ok(SecondaryManagers {
            fulltext: Some(manager),
            sync: Some(Arc::new(sync)),
        });
    }

    // Fulltext is off (or not compiled in): the sync manager exists only to
    // host the vector coordinator.
    #[cfg(feature = "vector")]
    {
        let Some(coordinator) = create_vector_coordinator(config, vector_runtime)? else {
            return Ok(SecondaryManagers {
                #[cfg(feature = "fulltext")]
                fulltext: None,
                sync: None,
            });
        };
        let mut sync = SyncManager::new_without_fulltext();
        sync = sync.with_vector_coordinator(coordinator);
        return Ok(SecondaryManagers {
            #[cfg(feature = "fulltext")]
            fulltext: None,
            sync: Some(Arc::new(sync)),
        });
    }
    #[cfg(not(feature = "vector"))]
    {
        Ok(SecondaryManagers {
            #[cfg(feature = "fulltext")]
            fulltext: None,
            sync: None,
        })
    }
}

/// Embedded Linkrs database
///
/// This is the main entry point for the embedded API, offering a simple way of use similar to that of SQLite.
/// The sqlite3 structure corresponding to SQLite.
///
/// # Example
///
/// ```rust
/// use linkrs_api::embedded::{GraphDatabase, DatabaseConfig};
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
pub struct GraphDatabase<S: StorageClient + Clone + Send + Sync + 'static> {
    pub(crate) inner: Arc<GraphDatabaseInner<S>>,
    config: Arc<RwLock<DatabaseConfig>>,
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
            let sync_policy = Some(config.sync_mode.to_sync_policy());
            let mut persistence_config = PersistenceConfig::for_work_dir(path);
            persistence_config.enable_wal = enable_wal;
            persistence_config.sync_policy = sync_policy;
            persistence_config.checkpoint_on_close = config.checkpoint_on_close;
            let storage =
                GraphStorage::open_with_persistence_config(path.to_path_buf(), persistence_config)
                    .map_err(|e| {
                        CoreError::StorageError(format!("Failed to initialize storage: {}", e))
                    })?;
            (storage, enable_wal, sync_policy)
        };

        let version_manager = storage.version_manager();
        let storage = Arc::new(RwLock::new(storage));

        let managers = {
            #[cfg(feature = "vector")]
            let vector_runtime = Some(vector_runtime.handle());
            #[cfg(not(feature = "vector"))]
            let vector_runtime = None::<&tokio::runtime::Handle>;
            build_secondary_managers(&config, &FulltextConfig::default(), vector_runtime)?
        };
        #[cfg(feature = "fulltext")]
        let fulltext_manager = managers.fulltext;
        let mut sync_manager = managers.sync;

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
            // Fail stranded rebuild generations and collect rebuild scratch
            // state left by a previous process. Idempotent; best-effort here
            // (recovery is re-runnable on demand).
            if let Err(error) = manager.recover_stale_rebuilds_sync() {
                log::warn!("Rebuild startup recovery reported an error: {}", error);
            }
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
            Arc::new(RwLock::new(
                QueryApi::new(storage.clone(), stats_manager.clone())
                    .with_sync_manager(sync.clone()),
            ))
        } else {
            Arc::new(RwLock::new(QueryApi::new(
                storage.clone(),
                stats_manager.clone(),
            )))
        };
        let schema_api = SchemaApi::new(storage.clone());

        let hooks = crate::embedded::hooks::HookBus::with_txn_manager(
            &txn_manager,
            Some(storage.read().shared_schema_callbacks()),
        );
        // Aggregate leaf registries so one bus subscription covers the whole
        // backend. Each attach is best-effort: backends without a coordinator
        // (in-memory) simply leave the domain unattached (`has_*_registry`).
        if let Some(shared) = storage.read().shared_storage_callbacks() {
            hooks.attach_storage_registry(shared);
        }
        #[cfg(feature = "fulltext")]
        if let Some(ref manager) = fulltext_manager {
            hooks.attach_index_registry(manager.shared_index_callbacks());
        }

        let inner = Arc::new(GraphDatabaseInner {
            query_api,
            schema_api,
            txn_manager,
            storage,
            sync_manager,
            stats_manager,
            hooks,
            #[cfg(feature = "vector")]
            vector_runtime,
            read_only: config.read_only,
        });

        Ok(Self {
            inner,
            config: Arc::new(RwLock::new(config)),
        })
    }

    /// Idempotent close: quiesce background loops and, when
    /// `checkpoint_on_close` is configured, persist one final checkpoint.
    ///
    /// Forwards to the storage close with a read lock; close failures are
    /// logged inside storage and never surface. After close, operations fail
    /// with the storage-not-open error.
    pub fn close(&self) {
        self.inner.storage.read().close();
    }
}

impl<S: StorageClient + Clone + Send + Sync + 'static> GraphDatabase<S> {
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

    /// Create a tag in `space` (convenience method over `Session::create_tag`).
    pub fn create_tag(
        &self,
        space: &str,
        name: &str,
        properties: Vec<crate::api_core::types::PropertyDef>,
    ) -> CoreResult<()> {
        let session = self.session()?;
        session.create_tag(space, name, properties)
    }

    /// Drop a tag from `space` (convenience method).
    pub fn drop_tag(&self, space: &str, name: &str) -> CoreResult<()> {
        let session = self.session()?;
        session.drop_tag(space, name)
    }

    /// List tag names in `space` (convenience method).
    pub fn list_tags(&self, space: &str) -> CoreResult<Vec<String>> {
        let session = self.session()?;
        session.list_tags(space)
    }

    /// Create an edge type in `space` (convenience method).
    pub fn create_edge_type(
        &self,
        space: &str,
        name: &str,
        properties: Vec<crate::api_core::types::PropertyDef>,
    ) -> CoreResult<()> {
        let session = self.session()?;
        session.create_edge_type(space, name, properties)
    }

    /// Drop an edge type from `space` (convenience method).
    pub fn drop_edge_type(&self, space: &str, name: &str) -> CoreResult<()> {
        let session = self.session()?;
        session.drop_edge_type(space, name)
    }

    /// List edge type names in `space` (convenience method).
    pub fn list_edge_types(&self, space: &str) -> CoreResult<Vec<String>> {
        let session = self.session()?;
        session.list_edge_types(space)
    }

    /// Fetch full descriptor for `space` (`None` when missing).
    ///
    /// Embedded equivalent of `GET /v1/spaces/{name}` and gRPC `GetSpace`.
    pub fn get_space(&self, space: &str) -> CoreResult<Option<linkrs_core::types::SpaceInfo>> {
        let session = self.session()?;
        session.get_space(space)
    }

    /// Fetch full descriptor for tag `name` in `space` (`None` when missing).
    pub fn get_tag(
        &self,
        space: &str,
        name: &str,
    ) -> CoreResult<Option<linkrs_core::types::TagInfo>> {
        let session = self.session()?;
        session.get_tag(space, name)
    }

    /// Fetch full descriptor for edge type `name` in `space` (`None` when missing).
    pub fn get_edge_type(
        &self,
        space: &str,
        name: &str,
    ) -> CoreResult<Option<linkrs_core::types::EdgeTypeInfo>> {
        let session = self.session()?;
        session.get_edge_type(space, name)
    }

    /// Version history of a vertex tag (embedded equivalent of
    /// `GET /v1/schema/versions/{space}/{label}`).
    pub fn get_vertex_version_history(
        &self,
        space: &str,
        tag: &str,
    ) -> CoreResult<Option<linkrs_storage::LabelVersionHistory>> {
        let session = self.session()?;
        session.get_vertex_version_history(space, tag)
    }

    /// Version history of an edge type.
    pub fn get_edge_version_history(
        &self,
        space: &str,
        edge_type: &str,
    ) -> CoreResult<Option<linkrs_storage::LabelVersionHistory>> {
        let session = self.session()?;
        session.get_edge_version_history(space, edge_type)
    }

    /// Schema changes between two versions of a label.
    pub fn get_schema_changes(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
        from_version: u64,
        to_version: u64,
    ) -> CoreResult<Vec<linkrs_storage::PropertyChange>> {
        let session = self.session()?;
        session.get_schema_changes(space, label, is_edge, from_version, to_version)
    }

    /// Breaking changes between two versions of a label.
    pub fn detect_breaking_changes(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
        from_version: u64,
        to_version: u64,
    ) -> CoreResult<Vec<linkrs_storage::PropertyChange>> {
        let session = self.session()?;
        session.detect_breaking_changes(space, label, is_edge, from_version, to_version)
    }

    /// Validate query text without executing it (convenience method).
    ///
    /// Mirrors `POST /v1/query/validate`: parses and binds the statement,
    /// returning parameter names on success.
    pub fn validate_query(
        &self,
        query: &str,
    ) -> CoreResult<crate::embedded::session::validate::QueryValidation> {
        let session = self.session()?;
        session.validate_query(query)
    }

    /// Plan a statement via `EXPLAIN` without executing it (convenience method).
    ///
    /// Mirrors `POST /v1/query/explain`.
    pub fn explain_query(&self, query: &str) -> CoreResult<QueryResult> {
        let session = self.session()?;
        session.explain_query(query)
    }

    /// List currently active transactions (convenience method).
    ///
    /// Embedded equivalent of `GET /v1/transactions`.
    pub fn list_transactions(&self) -> Vec<linkrs_transaction::TransactionInfo> {
        self.inner.txn_manager.list_transactions()
    }

    /// Abort another transaction by id (convenience method).
    ///
    /// Embedded equivalent of `POST /v1/transactions/{id}/kill`.
    pub fn kill_transaction(&self, txn_id: u64) -> CoreResult<()> {
        let session = self.session()?;
        session.kill_transaction(txn_id)
    }

    /// Execute `query` and write the result to `path` as `csv`/`jsonl`.
    ///
    /// Embedded equivalent of `GET /v1/export`. Returns rows written.
    pub fn export_query_to_file(
        &self,
        query: &str,
        path: impl AsRef<std::path::Path>,
        format: crate::embedded::session::export::ExportFormat,
    ) -> CoreResult<usize> {
        let session = self.session()?;
        session.export_query_to_file(query, path, format)
    }

    /// Simplified database overview for embedded use (subset of
    /// `GET /v1/statistics/overview`).
    ///
    /// Combines space count, live transaction metrics, and the shared
    /// `StatsManager` query counters so single-process users get the same
    /// top-level signals as the server overview without the system/storage
    /// sections (which need a server runtime).
    pub fn database_overview(&self) -> serde_json::Value {
        let spaces = self.list_spaces().map(|names| names.len()).unwrap_or(0);
        let metrics = self.transaction_metrics();
        let counters = self.inner.stats_manager.get_all_metrics();
        let counters: std::collections::HashMap<String, u64> = counters
            .into_iter()
            .map(|(k, v)| (format!("{k:?}"), v))
            .collect();
        serde_json::json!({
            "spaces": spaces,
            "transactions": {
                "begun": metrics.begun,
                "active": metrics.active,
                "committed": metrics.committed,
                "aborted": metrics.aborted,
                "active_statements": metrics.active_statements,
                "active_snapshots": metrics.active_snapshots,
                "pending_writes": metrics.pending_writes,
            },
            "counters": counters,
            "in_memory": self.is_memory(),
        })
    }

    /// Snapshot transaction counters and resource gauges (convenience method).
    ///
    /// Embedded equivalent of `GET /v1/transactions/metrics`.
    pub fn transaction_metrics(&self) -> crate::embedded::transaction::TransactionMetrics {
        use std::sync::atomic::Ordering;
        let stats = self.inner.txn_manager.stats();
        let resources = self.inner.txn_manager.resource_metrics();
        crate::embedded::transaction::TransactionMetrics {
            begun: stats.total_transactions.load(Ordering::Relaxed),
            active: stats.active_transactions.load(Ordering::Relaxed),
            committed: stats.committed_transactions.load(Ordering::Relaxed),
            aborted: stats.aborted_transactions.load(Ordering::Relaxed),
            active_statements: stats.active_statements.load(Ordering::Relaxed),
            active_snapshots: resources.active_snapshots,
            pending_writes: resources.pending_writes,
            staged_wal_bytes: resources.staged_wal_bytes,
            undo_bytes: resources.undo_bytes,
        }
    }

    /// Get Configuration (read guard; derefs to `&DatabaseConfig`).
    ///
    /// The guard keeps the existing `db.config().field` call style working
    /// while allowing live updates via `update_config_value` /
    /// `bulk_update_config` on the same instance.
    pub fn config(&self) -> parking_lot::RwLockReadGuard<'_, DatabaseConfig> {
        self.config.read()
    }

    /// Snapshot the current configuration (cloned value).
    pub fn config_snapshot(&self) -> DatabaseConfig {
        self.config.read().clone()
    }

    /// Read one `section.key` value as a string.
    ///
    /// Embedded equivalent of `GET /v1/config/{section}/{key}`.
    /// Supported keys: `cache.size_mb`, `timeout.default_ms`,
    /// `wal.enabled`, `sync.mode`, `storage.read_only`,
    /// `storage.create_if_missing`, `storage.checkpoint_on_close`.
    /// Returns `None` for unknown keys.
    pub fn get_config_value(&self, section: &str, key: &str) -> Option<String> {
        let config = self.config.read();
        Some(match (section, key) {
            ("cache", "size_mb") => config.cache_size_mb.to_string(),
            ("timeout", "default_ms") => config.default_timeout.as_millis().to_string(),
            ("wal", "enabled") => config.enable_wal.to_string(),
            ("sync", "mode") => format!("{:?}", config.sync_mode).to_lowercase(),
            ("storage", "read_only") => config.read_only.to_string(),
            ("storage", "create_if_missing") => config.create_if_missing.to_string(),
            ("storage", "checkpoint_on_close") => config.checkpoint_on_close.to_string(),
            _ => return None,
        })
    }

    /// Update one `section.key` value from its string form.
    ///
    /// Embedded equivalent of `PUT /v1/config/{section}/{key}`.
    /// Applied fields take effect immediately for session admission
    /// (`storage.read_only`) and for newly created sessions/transactions
    /// (`cache.size_mb`, `timeout.default_ms`); WAL/sync/persistence flags
    /// are recorded and apply on the next open (same as server restart
    /// semantics for those flags).
    pub fn update_config_value(&self, section: &str, key: &str, value: &str) -> CoreResult<()> {
        let mut config = self.config.write();
        match (section, key) {
            ("cache", "size_mb") => {
                config.cache_size_mb = value.parse::<usize>().map_err(|_| {
                    CoreError::InvalidParameter(format!("invalid cache.size_mb: {value}"))
                })?;
            }
            ("timeout", "default_ms") => {
                let ms = value.parse::<u64>().map_err(|_| {
                    CoreError::InvalidParameter(format!("invalid timeout.default_ms: {value}"))
                })?;
                config.default_timeout = std::time::Duration::from_millis(ms);
            }
            ("wal", "enabled") => {
                config.enable_wal = value.parse::<bool>().map_err(|_| {
                    CoreError::InvalidParameter(format!("invalid wal.enabled: {value}"))
                })?;
            }
            ("sync", "mode") => {
                config.sync_mode = match value.to_lowercase().as_str() {
                    "normal" => linkrs_config::SyncMode::Normal,
                    "full" => linkrs_config::SyncMode::Full,
                    _ => {
                        return Err(CoreError::InvalidParameter(format!(
                            "invalid sync.mode: {value} (expected normal|full)"
                        )));
                    }
                };
            }
            ("storage", "read_only") => {
                config.read_only = value.parse::<bool>().map_err(|_| {
                    CoreError::InvalidParameter(format!("invalid storage.read_only: {value}"))
                })?;
            }
            ("storage", "create_if_missing") => {
                config.create_if_missing = value.parse::<bool>().map_err(|_| {
                    CoreError::InvalidParameter(format!(
                        "invalid storage.create_if_missing: {value}"
                    ))
                })?;
            }
            ("storage", "checkpoint_on_close") => {
                config.checkpoint_on_close = value.parse::<bool>().map_err(|_| {
                    CoreError::InvalidParameter(format!(
                        "invalid storage.checkpoint_on_close: {value}"
                    ))
                })?;
            }
            _ => {
                return Err(CoreError::InvalidParameter(format!(
                    "unknown config key {section}.{key}"
                )));
            }
        }
        Ok(())
    }

    /// Reset one `section.key` to its compiled default.
    ///
    /// Embedded equivalent of `DELETE /v1/config/{section}/{key}`
    /// (server `ResetConfig`).
    pub fn reset_config_value(&self, section: &str, key: &str) -> CoreResult<()> {
        let defaults = DatabaseConfig::default();
        let fallback = match (section, key) {
            ("cache", "size_mb") => defaults.cache_size_mb.to_string(),
            ("timeout", "default_ms") => defaults.default_timeout.as_millis().to_string(),
            ("wal", "enabled") => defaults.enable_wal.to_string(),
            ("sync", "mode") => format!("{:?}", defaults.sync_mode).to_lowercase(),
            ("storage", "read_only") => defaults.read_only.to_string(),
            ("storage", "create_if_missing") => defaults.create_if_missing.to_string(),
            ("storage", "checkpoint_on_close") => defaults.checkpoint_on_close.to_string(),
            _ => {
                return Err(CoreError::InvalidParameter(format!(
                    "unknown config key {section}.{key}"
                )));
            }
        };
        self.update_config_value(section, key, &fallback)
    }

    /// Apply many `section.key -> value` updates atomically (best effort
    /// order; first failure aborts and returns the error).
    ///
    /// Embedded equivalent of `PUT /v1/config` (bulk) and gRPC
    /// `BulkUpdateConfig`.
    pub fn bulk_update_config(
        &self,
        updates: std::collections::HashMap<String, String>,
    ) -> CoreResult<()> {
        for (dotted, value) in &updates {
            let (section, key) = dotted.split_once('.').ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "invalid config key {dotted:?} (expected section.key)"
                ))
            })?;
            self.update_config_value(section, key, value)?;
        }
        Ok(())
    }

    /// Checking for in-memory databases
    pub fn is_memory(&self) -> bool {
        self.config.read().is_memory()
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

    /// Sync operations API.
    ///
    /// Returns `None` when synchronization is not configured; callers must
    /// not treat an absent manager as an empty outbox.
    pub fn sync_api(&self) -> Option<crate::api_core::SyncApi> {
        self.inner
            .sync_manager
            .clone()
            .map(crate::api_core::SyncApi::new)
    }

    /// Get stats manager
    pub fn stats_manager(&self) -> Arc<StatsManager> {
        self.inner.stats_manager.clone()
    }

    /// Get transaction manager for online coordination (drain fences, etc).
    pub(crate) fn txn_manager(&self) -> Arc<TransactionManager> {
        self.inner.txn_manager.clone()
    }

    /// Central event-hook facade: one subscription point for schema and
    /// transaction notifications (notification hooks only; the C-API
    /// `commit_hook` veto is separate).
    pub fn hooks(&self) -> &crate::embedded::hooks::HookBus {
        &self.inner.hooks
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

        let hooks = crate::embedded::hooks::HookBus::with_txn_manager(&txn_manager, None);

        let inner = Arc::new(GraphDatabaseInner {
            query_api,
            schema_api,
            txn_manager,
            storage,
            sync_manager: None,
            stats_manager,
            hooks,
            #[cfg(feature = "vector")]
            vector_runtime,
            read_only: false,
        });

        Ok(Self {
            inner,
            config: Arc::new(RwLock::new(DatabaseConfig::default())),
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

    #[test]
    fn test_close_with_checkpoint_on_close_writes_baseline() {
        let directory = tempfile::TempDir::new().unwrap();
        let db_path = directory.path().join("close.db");
        let db = GraphDatabase::open_with_config(
            DatabaseConfig::file(&db_path).with_checkpoint_on_close(true),
        )
        .unwrap();
        let mut session = db.session().unwrap();
        db.create_space("close_space", SpaceConfig::default())
            .unwrap();
        session.use_space("close_space").unwrap();
        session
            .execute("CREATE TAG close_probe(name string)")
            .unwrap();
        db.close();
        db.close();
        assert!(
            db_path.join("checkpoint").join("checkpoint_1").is_dir(),
            "close with the flag must persist one final checkpoint"
        );
        assert!(
            db.session()
                .unwrap()
                .execute("CREATE TAG after_close(name string)")
                .is_err(),
            "operations after close must fail with storage-not-open"
        );
    }

    #[test]
    fn test_close_without_checkpoint_on_close_skips_checkpoint() {
        let directory = tempfile::TempDir::new().unwrap();
        let db_path = directory.path().join("close.db");
        let db = GraphDatabase::open_with_config(DatabaseConfig::file(&db_path)).unwrap();
        assert!(!db.config().checkpoint_on_close);
        db.close();
        db.close();
        assert!(
            !db_path.join("checkpoint").join("checkpoint_1").exists(),
            "close without the flag must not write a checkpoint"
        );
    }

    #[test]
    fn test_read_only_database_rejects_writes() {
        let directory = tempfile::TempDir::new().unwrap();
        let db_path = directory.path().join("ro.db");

        let db =
            GraphDatabase::open_with_config(DatabaseConfig::file(&db_path).with_read_only(true))
                .unwrap();
        let session = db.session().unwrap();

        let err = session.execute("DROP SPACE nosuch").unwrap_err();
        assert!(
            err.to_string().contains("read-only"),
            "unexpected error: {}",
            err
        );

        assert!(session.execute("RETURN 1").is_ok());

        assert!(session.begin_transaction().is_err());
        let txn = session
            .begin_transaction_with_config(crate::embedded::TransactionConfig::new().read_only())
            .unwrap();
        txn.rollback().unwrap();
    }

    #[cfg(feature = "fulltext")]
    #[test]
    fn secondary_managers_skip_fulltext_when_disabled() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = DatabaseConfig::file(directory.path().join("graph.db"));
        let managers = build_secondary_managers(&config, &FulltextConfig::default(), None).unwrap();
        assert!(
            managers.fulltext.is_none(),
            "default fulltext config is disabled"
        );
    }

    #[cfg(feature = "fulltext")]
    #[test]
    fn secondary_managers_surface_setup_failures() {
        let directory = tempfile::TempDir::new().unwrap();
        // A regular file cannot host the index directory, so fulltext setup
        // must fail loudly instead of being silently skipped.
        let regular_file = directory.path().join("not-a-dir");
        std::fs::write(&regular_file, b"x").unwrap();
        let mut fulltext_config = FulltextConfig::default();
        fulltext_config.enabled = true;
        fulltext_config.index_path = regular_file.join("index");
        let config = DatabaseConfig::file(directory.path().join("graph.db"));

        let err = build_secondary_managers(&config, &fulltext_config, None)
            .err()
            .expect("invalid index path must fail the setup");
        assert!(
            matches!(err, CoreError::Internal(ref msg) if !msg.is_empty()),
            "setup failure must surface as an internal error: {err}"
        );
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
            backend.as_ref().and_then(|b| b.as_local()).is_some(),
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
