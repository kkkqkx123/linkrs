//! Local vector engine.
//!
//! Owns one collection directory per collection under a root directory and
//! keeps an in-memory registry of opened [`CollectionStore`]s. This is the
//! transport-independent engine surface; the graphdb-sync coordinator wraps it
//! in an async shell (`VectorBackend::Local`).
//!
//! Every mutation is WAL-backed (see [`CollectionStore::apply_txn`]):
//! append + fsync before applying to memory, so crash recovery replays
//! idempotently. Coordinated graph transactions land in [`LocalVectorEngine::apply_txn`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::error::{Result, VectorSearchError};
use crate::storage::{CollectionStore, WalPoint, WalRecord, WalTxn};
use crate::types::{
    CollectionConfig, CollectionInfo, CollectionStatus, PointId, SearchQuery, SearchResult,
    VectorFilter, VectorPoint,
};

/// A single operation of a coordinated transaction, grouped per collection.
#[derive(Debug, Clone)]
pub enum TxnOp {
    /// Upsert a point into a collection.
    Upsert {
        collection: String,
        point: VectorPoint,
    },
    /// Delete a point by id from a collection.
    Delete {
        collection: String,
        point_id: String,
    },
}

/// The built-in (local) vector engine.
///
/// All operations are synchronous; the graphdb-sync coordinator serializes
/// access through an async shell. Collection names must be valid path segments
/// (enforced by [`CollectionStore::create`]).
pub struct LocalVectorEngine {
    root_dir: PathBuf,
    collections: RwLock<HashMap<String, Arc<CollectionStore>>>,
}

impl std::fmt::Debug for LocalVectorEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalVectorEngine")
            .field("root_dir", &self.root_dir)
            .field("collection_count", &self.collections.read().len())
            .finish()
    }
}

impl LocalVectorEngine {
    /// Open (or create) the engine root directory, loading every existing
    /// collection. Returns [`VectorSearchError::InvalidConfig`] if a
    /// collection directory is corrupt.
    pub fn open(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&root_dir)?;

        let mut collections = HashMap::new();
        for entry in std::fs::read_dir(&root_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("meta.bin").exists() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let store = Arc::new(CollectionStore::open(&path)?);
            collections.insert(name, store);
        }

        Ok(Self {
            root_dir,
            collections: RwLock::new(collections),
        })
    }

    /// Root directory backing this engine.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Names of all loaded collections.
    pub fn collection_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.collections.read().keys().cloned().collect();
        names.sort_unstable();
        names
    }

    /// Create a collection. Fails with
    /// [`VectorSearchError::CollectionAlreadyExists`] if it already exists.
    pub fn create_collection(&self, name: &str, config: &CollectionConfig) -> Result<()> {
        let dir = self.root_dir.join(name);
        let store = Arc::new(CollectionStore::create(&dir, name, config)?);
        self.collections.write().insert(name.to_string(), store);
        Ok(())
    }

    /// Drop a collection and delete its directory.
    pub fn delete_collection(&self, name: &str) -> Result<()> {
        let dir = {
            let mut collections = self.collections.write();
            let store = collections
                .remove(name)
                .ok_or_else(|| VectorSearchError::CollectionNotFound(name.to_string()))?;
            store.dir().to_path_buf()
        };
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// Whether a collection exists.
    pub fn collection_exists(&self, name: &str) -> bool {
        self.collections.read().contains_key(name)
    }

    /// Collection configuration, or `None` if the collection does not exist.
    ///
    /// The local engine persists only dimension + distance; the remaining
    /// [`CollectionConfig`] fields (HNSW/quantization/etc.) are not stored.
    pub fn collection_config(&self, name: &str) -> Result<Option<CollectionConfig>> {
        let collections = self.collections.read();
        Ok(collections.get(name).map(|store| {
            let meta = store.meta();
            CollectionConfig::new(meta.vector_size, meta.distance)
        }))
    }

    /// Detailed collection info.
    pub fn collection_info(&self, name: &str) -> Result<CollectionInfo> {
        let collections = self.collections.read();
        let store = collections
            .get(name)
            .ok_or_else(|| VectorSearchError::CollectionNotFound(name.to_string()))?;
        let meta = store.meta();
        let live = store.count();
        let segments = (meta.next_slot as usize).div_ceil(meta.segment_slots as usize);
        Ok(CollectionInfo {
            name: meta.collection.clone(),
            vector_count: live,
            indexed_vector_count: live,
            points_count: live,
            segments_count: segments as u64,
            config: CollectionConfig::new(meta.vector_size, meta.distance),
            status: CollectionStatus::Green,
        })
    }

    /// Upsert a point (WAL-backed).
    pub fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()> {
        let store = self.store(collection)?;
        store.apply_ops(&[WalRecord::Upsert {
            point: WalPoint::from_point(&point)?,
        }])
    }

    /// Upsert a batch of points (WAL-backed, single transaction).
    pub fn upsert_batch(&self, collection: &str, points: &[VectorPoint]) -> Result<()> {
        let store = self.store(collection)?;
        let ops: Result<Vec<_>> = points
            .iter()
            .map(|p| {
                Ok(WalRecord::Upsert {
                    point: WalPoint::from_point(p)?,
                })
            })
            .collect();
        store.apply_ops(&ops?)
    }

    /// Delete a point by id (WAL-backed). Deleting a missing id is a no-op.
    pub fn delete(&self, collection: &str, point_id: &str) -> Result<()> {
        let store = self.store(collection)?;
        store.apply_ops(&[WalRecord::Delete {
            point_id: point_id.to_string(),
        }])
    }

    /// Delete a batch of points (WAL-backed, single transaction).
    pub fn delete_batch(&self, collection: &str, point_ids: &[String]) -> Result<()> {
        let store = self.store(collection)?;
        store.apply_ops(&[WalRecord::DeleteBatch {
            point_ids: point_ids.to_vec(),
        }])
    }

    /// Delete every point matching `filter`. Returns the number deleted.
    pub fn delete_by_filter(&self, collection: &str, filter: &VectorFilter) -> Result<u64> {
        self.store(collection)?.delete_by_filter(filter)
    }

    /// Exact full-scan search (Tier 0). Scores follow Qdrant semantics:
    /// cosine similarity, inner product and 1/(1+distance) for euclid.
    pub fn search(&self, collection: &str, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        self.store(collection)?.search(query)
    }

    /// Fetch a point by id.
    pub fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        let store = self.store(collection)?;
        store.get(&PointId::from(point_id.to_string()))
    }

    /// Number of live points in a collection.
    pub fn count(&self, collection: &str) -> Result<u64> {
        Ok(self.store(collection)?.count())
    }

    /// Commit protocol entry for coordinated transactions.
    ///
    /// Ops are grouped by collection and each collection receives one WAL
    /// transaction carrying `txn_id`. Collections are processed in
    /// lexicographic order so a crash always advances the per-collection water
    /// marks in a deterministic order. Replay is idempotent, so graph WAL
    /// replay may re-apply the same txn id without double-applying data.
    pub fn apply_txn(&self, txn_id: u64, ops: Vec<TxnOp>) -> Result<()> {
        let mut by_collection: HashMap<String, Vec<WalRecord>> = HashMap::new();
        for op in ops {
            match op {
                TxnOp::Upsert { collection, point } => by_collection
                    .entry(collection)
                    .or_default()
                    .push(WalRecord::Upsert {
                        point: WalPoint::from_point(&point)?,
                    }),
                TxnOp::Delete {
                    collection,
                    point_id,
                } => by_collection
                    .entry(collection)
                    .or_default()
                    .push(WalRecord::Delete { point_id }),
            }
        }

        let mut names: Vec<String> = by_collection.keys().cloned().collect();
        names.sort_unstable();
        for name in names {
            let ops = by_collection.remove(&name).expect("key present");
            let store = self.store(&name)?;
            store.apply_txn(&WalTxn { txn_id, ops })?;
        }
        Ok(())
    }

    fn store(&self, collection: &str) -> Result<Arc<CollectionStore>> {
        self.collections
            .read()
            .get(collection)
            .cloned()
            .ok_or_else(|| VectorSearchError::CollectionNotFound(collection.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DistanceMetric, FilterCondition, Payload, VectorFilter};

    fn config(dim: usize) -> CollectionConfig {
        CollectionConfig::new(dim, DistanceMetric::Cosine)
    }

    fn point(id: u64, dim: usize) -> VectorPoint {
        VectorPoint::new(
            id,
            (0..dim)
                .map(|i| ((id as usize * 31 + i) % 100) as f32 / 100.0)
                .collect(),
        )
    }

    fn point_with_color(id: u64, dim: usize, color: &str) -> VectorPoint {
        let mut payload: Payload = HashMap::new();
        payload.insert("color".to_string(), serde_json::json!(color));
        VectorPoint::new(id, (0..dim).map(|_| 0.5).collect()).with_payload(payload)
    }

    fn engine() -> LocalVectorEngine {
        let dir = tempfile::tempdir().unwrap();
        LocalVectorEngine::open(dir.path().join("vec")).unwrap()
    }

    #[test]
    fn test_create_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vec");
        {
            let engine = LocalVectorEngine::open(&root).unwrap();
            engine.create_collection("col_a", &config(4)).unwrap();
            engine.upsert("col_a", point(1, 4)).unwrap();
            engine.upsert("col_a", point(2, 4)).unwrap();
            assert_eq!(engine.count("col_a").unwrap(), 2);
        }
        let reopened = LocalVectorEngine::open(&root).unwrap();
        assert!(reopened.collection_exists("col_a"));
        assert_eq!(reopened.count("col_a").unwrap(), 2);
        let got = reopened.get("col_a", "1").unwrap().unwrap();
        assert_eq!(got.vector, point(1, 4).vector);
        assert!(reopened.get("col_a", "99").unwrap().is_none());
    }

    #[test]
    fn test_create_duplicate_fails() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        let err = engine.create_collection("col", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::CollectionAlreadyExists(_)));
    }

    #[test]
    fn test_invalid_collection_name() {
        let engine = engine();
        let err = engine.create_collection("a/b", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::InvalidCollectionName(_)));
        let err = engine.create_collection("", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::InvalidCollectionName(_)));
    }

    #[test]
    fn test_delete_collection() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vec");
        {
            let engine = LocalVectorEngine::open(&root).unwrap();
            engine.create_collection("col", &config(4)).unwrap();
            engine.upsert("col", point(1, 4)).unwrap();
            engine.delete_collection("col").unwrap();
            assert!(!engine.collection_exists("col"));
        }
        let reopened = LocalVectorEngine::open(&root).unwrap();
        assert!(!reopened.collection_exists("col"));
    }

    #[test]
    fn test_delete_collection_missing() {
        let engine = engine();
        let err = engine.delete_collection("nope").unwrap_err();
        assert!(matches!(err, VectorSearchError::CollectionNotFound(_)));
    }

    #[test]
    fn test_collection_config_and_info() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        let cfg = engine.collection_config("col").unwrap().unwrap();
        assert_eq!(cfg.vector_size, 4);
        assert_eq!(cfg.distance, DistanceMetric::Cosine);
        assert!(engine.collection_config("nope").unwrap().is_none());

        engine.upsert("col", point(1, 4)).unwrap();
        let info = engine.collection_info("col").unwrap();
        assert_eq!(info.name, "col");
        assert_eq!(info.points_count, 1);
        assert_eq!(info.vector_count, 1);
        assert_eq!(info.segments_count, 1);
        assert_eq!(info.status, CollectionStatus::Green);
    }

    #[test]
    fn test_search_returns_topk() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        engine.upsert("col", point(1, 4)).unwrap();
        engine.upsert("col", point(2, 4)).unwrap();
        engine.upsert("col", point(3, 4)).unwrap();

        let results = engine
            .search("col", &SearchQuery::new(vec![0.0; 4], 2).with_payload(true))
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results[0].score >= results[1].score,
            "results sorted descending: {:?}",
            results
        );
    }

    #[test]
    fn test_search_dimension_mismatch() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        engine.upsert("col", point(1, 4)).unwrap();
        let err = engine
            .search("col", &SearchQuery::new(vec![1.0, 2.0], 1))
            .unwrap_err();
        assert!(matches!(
            err,
            VectorSearchError::InvalidVectorDimension {
                expected: 4,
                actual: 2
            }
        ));
    }

    #[test]
    fn test_delete_by_id() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        engine.upsert("col", point(1, 4)).unwrap();
        engine.upsert("col", point(2, 4)).unwrap();
        engine.delete("col", "1").unwrap();
        assert_eq!(engine.count("col").unwrap(), 1);
        assert!(engine.get("col", "1").unwrap().is_none());
        engine.delete("col", "99").unwrap();
        assert_eq!(engine.count("col").unwrap(), 1);
    }

    #[test]
    fn test_delete_by_filter() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        engine.upsert("col", point_with_color(1, 4, "red")).unwrap();
        engine
            .upsert("col", point_with_color(2, 4, "blue"))
            .unwrap();
        engine.upsert("col", point_with_color(3, 4, "red")).unwrap();
        assert_eq!(engine.count("col").unwrap(), 3);

        let filter = VectorFilter::new().must(FilterCondition::match_value("color", "red"));
        let deleted = engine.delete_by_filter("col", &filter).unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(engine.count("col").unwrap(), 1);
        assert!(engine.get("col", "2").unwrap().is_some());
        assert!(engine.get("col", "1").unwrap().is_none());
        assert!(engine.get("col", "3").unwrap().is_none());
    }

    #[test]
    fn test_delete_by_filter_no_match() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        engine.upsert("col", point(1, 4)).unwrap();
        let filter = VectorFilter::new().must(FilterCondition::match_value("color", "red"));
        assert_eq!(engine.delete_by_filter("col", &filter).unwrap(), 0);
        assert_eq!(engine.count("col").unwrap(), 1);
    }

    #[test]
    fn test_apply_txn_multi_collection() {
        let engine = engine();
        engine.create_collection("col_b", &config(4)).unwrap();
        engine.create_collection("col_a", &config(4)).unwrap();

        let ops = vec![
            TxnOp::Upsert {
                collection: "col_b".to_string(),
                point: point(1, 4),
            },
            TxnOp::Upsert {
                collection: "col_a".to_string(),
                point: point(2, 4),
            },
            TxnOp::Delete {
                collection: "col_a".to_string(),
                point_id: "42".to_string(),
            },
        ];
        engine.apply_txn(7, ops).unwrap();

        assert_eq!(engine.count("col_a").unwrap(), 1);
        assert_eq!(engine.count("col_b").unwrap(), 1);
        assert!(engine.get("col_b", "1").unwrap().is_some());

        // Replaying the same txn id is idempotent.
        engine
            .apply_txn(
                7,
                vec![TxnOp::Upsert {
                    collection: "col_a".to_string(),
                    point: point(2, 4),
                }],
            )
            .unwrap();
        assert_eq!(engine.count("col_a").unwrap(), 1);
    }

    #[test]
    fn test_apply_txn_unknown_collection() {
        let engine = engine();
        let err = engine
            .apply_txn(
                1,
                vec![TxnOp::Upsert {
                    collection: "nope".to_string(),
                    point: point(1, 4),
                }],
            )
            .unwrap_err();
        assert!(matches!(err, VectorSearchError::CollectionNotFound(_)));
    }

    #[test]
    fn test_apply_txn_validates_dimension() {
        let engine = engine();
        engine.create_collection("col", &config(4)).unwrap();
        let err = engine
            .apply_txn(
                1,
                vec![TxnOp::Upsert {
                    collection: "col".to_string(),
                    point: VectorPoint::new(1u64, vec![1.0, 2.0]),
                }],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            VectorSearchError::InvalidVectorDimension {
                expected: 4,
                actual: 2
            }
        ));
        assert_eq!(engine.count("col").unwrap(), 0);
    }

    #[test]
    fn test_wal_recovery_after_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vec");
        {
            let engine = LocalVectorEngine::open(&root).unwrap();
            engine.create_collection("col", &config(4)).unwrap();
            engine
                .apply_txn(
                    3,
                    vec![TxnOp::Upsert {
                        collection: "col".to_string(),
                        point: point(1, 4),
                    }],
                )
                .unwrap();
            engine.upsert("col", point(2, 4)).unwrap();
            engine.delete("col", "2").unwrap();
        }
        let reopened = LocalVectorEngine::open(&root).unwrap();
        assert_eq!(reopened.count("col").unwrap(), 1);
        assert!(reopened.get("col", "1").unwrap().is_some());
        assert!(reopened.get("col", "2").unwrap().is_none());
    }
}
