//! Vector backend abstraction.
//!
//! The sync layer talks to either the built-in local engine or the remote
//! qdrant client through a single enum. The local variant is always available
//! when the `vector` feature is enabled; the qdrant variant additionally
//! requires the `vector-qdrant` feature (which pulls in the network stack).

use std::sync::Arc;

use vector_search::{
    CollectionConfig, IndexMetadata, LocalVectorEngine, PayloadSchemaType, SearchQuery,
    SearchResult, TxnOp, VectorFilter, VectorPoint,
};

use crate::core::types::TransactionId;

#[allow(unused_imports)]
use super::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};

#[cfg(feature = "vector-qdrant")]
use vector_client::VectorManager;

/// The active vector engine.
#[derive(Clone)]
pub enum VectorBackend {
    /// Built-in local engine (synchronous, disk-backed).
    Local(Arc<LocalVectorEngine>),
    /// Remote qdrant client.
    #[cfg(feature = "vector-qdrant")]
    Qdrant(Arc<VectorManager>),
}

impl std::fmt::Debug for VectorBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorBackend::Local(_) => f.debug_tuple("Local").finish(),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(_) => f.debug_tuple("Qdrant").finish(),
        }
    }
}

impl VectorBackend {
    /// Whether the engine is the built-in local engine.
    pub fn is_local(&self) -> bool {
        matches!(self, VectorBackend::Local(_))
    }

    /// Whether the engine is unavailable (qdrant disabled engine).
    ///
    /// The local engine is never disabled: it is process-local and fails hard
    /// instead of degrading silently.
    pub fn is_disabled(&self) -> bool {
        match self {
            VectorBackend::Local(_) => false,
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.engine().name() == "disabled",
        }
    }

    /// Access the underlying local engine, if present.
    pub fn local(&self) -> Option<&Arc<LocalVectorEngine>> {
        match self {
            VectorBackend::Local(engine) => Some(engine),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(_) => None,
        }
    }

    /// Access the underlying qdrant manager, if present.
    #[cfg(feature = "vector-qdrant")]
    pub fn qdrant(&self) -> Option<&Arc<VectorManager>> {
        match self {
            VectorBackend::Local(_) => None,
            VectorBackend::Qdrant(manager) => Some(manager),
        }
    }

    // ---- collection management ----

    /// Create a collection. Fails if it already exists.
    pub async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => engine
                .create_collection(name, config)
                .map_err(VectorError::from)?,
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.create_index(name, config.clone()).await?,
        }
        Ok(())
    }

    /// Create a payload index for filter acceleration. Only meaningful for the
    /// remote engine; the local engine scans payloads directly (no-op).
    #[allow(unused_variables)]
    pub async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(_) => Ok(()),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .create_payload_index(collection, field, schema)
                    .await?;
                Ok(())
            }
        }
    }

    /// Whether a collection exists.
    pub fn index_exists(&self, name: &str) -> bool {
        match self {
            VectorBackend::Local(engine) => engine.collection_exists(name),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.index_exists(name),
        }
    }

    /// Collection metadata, or `None` when the collection does not exist.
    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        match self {
            VectorBackend::Local(engine) => {
                let config = engine.collection_config(name).ok().flatten()?;
                let vector_count = engine.count(name).ok()?;
                Some(IndexMetadata {
                    name: name.to_string(),
                    config,
                    created_at: chrono::Utc::now(),
                    vector_count,
                    index_name: None,
                })
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.get_index_metadata(name),
        }
    }

    /// Drop a collection.
    pub async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine.delete_collection(name).map_err(VectorError::from)?
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.drop_index(name).await?,
        }
        Ok(())
    }

    /// Fetch a single point by id.
    pub async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        match self {
            VectorBackend::Local(engine) => Ok(engine
                .get(collection, point_id)
                .map_err(VectorError::from)?),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager.get(collection, point_id).await?),
        }
    }

    /// Number of vectors in a collection.
    pub async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        match self {
            VectorBackend::Local(engine) => {
                Ok(engine.count(collection).map_err(VectorError::from)?)
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager.count(collection).await?),
        }
    }

    // ---- mutations ----

    /// Upsert a point.
    pub async fn upsert(
        &self,
        collection: &str,
        point: VectorPoint,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .upsert(collection, point)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.upsert(collection, point).await?;
                Ok(())
            }
        }
    }

    /// Upsert a batch of points.
    pub async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .upsert_batch(collection, &points)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.upsert_batch(collection, points).await?;
                Ok(())
            }
        }
    }

    /// Delete a point by id.
    pub async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .delete(collection, point_id)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete(collection, point_id).await?;
                Ok(())
            }
        }
    }

    /// Delete a batch of points by id.
    pub async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                let ids: Vec<String> = point_ids.iter().map(|s| s.to_string()).collect();
                engine
                    .delete_batch(collection, &ids)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete_batch(collection, point_ids.to_vec()).await?;
                Ok(())
            }
        }
    }

    /// Delete every point matching `filter`.
    pub async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .delete_by_filter(collection, &filter)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete_by_filter(collection, filter).await?;
                Ok(())
            }
        }
    }

    // ---- queries ----

    /// Full search.
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        match self {
            VectorBackend::Local(engine) => {
                let results = engine
                    .search(collection, &query)
                    .map_err(VectorError::from)?;
                Ok(results)
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                let results = manager.search(collection, query).await?;
                Ok(results)
            }
        }
    }

    // ---- coordinated commits ----

    /// Commit a coordinated transaction to the engine.
    ///
    /// Only the local backend uses the WAL commit protocol; the qdrant backend
    /// delivers buffered changes through the outbox instead
    /// ([`super::vector_sync::VectorSyncCoordinator::on_vector_change_batch`]).
    pub async fn apply_txn(
        &self,
        txn_id: TransactionId,
        ops: Vec<TxnOp>,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .apply_txn(txn_id.into(), ops)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(_) => Err(VectorCoordinatorError::InvalidOperation(
                "apply_txn is only supported by the local backend".to_string(),
            )),
        }
    }
}
