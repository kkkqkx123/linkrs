//! Per-collection storage assembly.
//!
//! A collection directory contains:
//! - `meta.bin`     postcard `Meta`
//! - `vectors.bin`  dense row-major f32, segmented mmap
//! - `keys.bin`     slot -> point id directory
//! - `payloads.bin` slot -> payload blob directory + tombstone flags
//! - `wal.bin`      (reserved for the WAL work item)
//!
//! Writes are serialized through the store's `RwLock`; readers snapshot the
//! mmap-backed `ArcSwap` views and can scan without the lock afterwards.

mod directory;
mod keys;
mod meta;
mod payloads;
mod vectors;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bitvec::prelude::*;
use parking_lot::RwLock;

pub(crate) use meta::Meta;

use crate::error::{Result, VectorSearchError};
use crate::types::{CollectionConfig, DistanceMetric, PointId, VectorPoint};

use self::keys::Keys;
use self::payloads::Payloads;
use self::vectors::Vectors;

/// In-memory mutable state, guarded by the store's `RwLock`.
struct StoreInner {
    meta: Meta,
    reverse: HashMap<PointId, u32>,
}

/// A single opened collection.
pub struct CollectionStore {
    dir: PathBuf,
    inner: RwLock<StoreInner>,
    /// Tombstone mirror (slot -> deleted) for lock-free scans.
    tombstones: ArcSwap<BitVec>,
    vectors: Vectors,
    keys: Keys,
    payloads: Payloads,
}

impl CollectionStore {
    /// Create a new collection directory and open it.
    pub fn create(dir: impl AsRef<Path>, collection: &str, config: &CollectionConfig) -> Result<Self> {
        Self::create_with_segment_slots(dir, collection, config, meta::SEGMENT_SLOTS_DEFAULT)
    }

    /// Like [`CollectionStore::create`] but with an explicit segment slot count.
    #[doc(hidden)]
    pub fn create_with_segment_slots(
        dir: impl AsRef<Path>,
        collection: &str,
        config: &CollectionConfig,
        segment_slots: u32,
    ) -> Result<Self> {
        validate_collection_name(collection)?;
        if config.vector_size == 0 {
            return Err(VectorSearchError::InvalidVectorDimension {
                expected: 1,
                actual: config.vector_size,
            });
        }
        if !matches!(
            config.distance,
            DistanceMetric::Cosine | DistanceMetric::Euclid | DistanceMetric::Dot
        ) {
            return Err(VectorSearchError::UnsupportedMetric(config.distance));
        }

        let dir = dir.as_ref();
        if dir.exists() {
            return Err(VectorSearchError::CollectionAlreadyExists(
                collection.to_string(),
            ));
        }
        std::fs::create_dir_all(dir)?;

        let meta = Meta::new_with_segment_slots(collection, config.vector_size, config.distance, segment_slots);
        meta.save(dir)?;

        let vectors = Vectors::create(&dir.join("vectors.bin"), config.vector_size, meta.segment_slots)?;
        let keys = Keys::create(&dir.join("keys.bin"), meta.slot_capacity)?;
        let payloads = Payloads::create(&dir.join("payloads.bin"), meta.slot_capacity)?;

        let tombstones = ArcSwap::from(Arc::new(bitvec::bitvec![0; meta.slot_capacity as usize]));
        Ok(Self {
            dir: dir.to_path_buf(),
            inner: RwLock::new(StoreInner {
                meta,
                reverse: HashMap::new(),
            }),
            tombstones,
            vectors,
            keys,
            payloads,
        })
    }

    /// Open an existing collection directory, rebuilding the id->slot map.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let meta = Meta::load(dir)?;

        let vectors = Vectors::open(&dir.join("vectors.bin"), meta.vector_size, meta.segment_slots, meta.slot_capacity)?;
        let keys = Keys::open(&dir.join("keys.bin"))?;
        let payloads = Payloads::open(&dir.join("payloads.bin"))?;

        let mut reverse = HashMap::new();
        let keys_view = keys.snapshot();
        let payloads_view = payloads.snapshot();
        for slot in 0..meta.next_slot as usize {
            if Payloads::is_tombstoned(&payloads_view, slot) {
                continue;
            }
            if let Some(key) = Keys::read_key(&keys_view, slot)? {
                let id = PointId::from(key);
                reverse.insert(id, slot as u32);
            }
        }
        drop(keys_view);
        drop(payloads_view);

        let mut tombstones = bitvec![0; meta.slot_capacity as usize];
        for slot in 0..meta.next_slot as usize {
            if Payloads::is_tombstoned(&payloads.snapshot(), slot) {
                tombstones.set(slot, true);
            }
        }

        Ok(Self {
            dir: dir.to_path_buf(),
            inner: RwLock::new(StoreInner { meta, reverse }),
            tombstones: ArcSwap::from(Arc::new(tombstones)),
            vectors,
            keys,
            payloads,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Snapshot of the metadata (dimension, metric, counts).
    pub fn meta(&self) -> Meta {
        self.inner.read().meta.clone()
    }

    /// Snapshot of the tombstone bitmap.
    pub fn tombstones(&self) -> arc_swap::Guard<Arc<BitVec>> {
        self.tombstones.load()
    }

    // Accessors consumed by the search pipeline (Tier 0 scan) and the engine
    // work item.
    #[allow(dead_code)]
    pub(crate) fn vectors(&self) -> &Vectors {
        &self.vectors
    }

    #[allow(dead_code)]
    pub(crate) fn keys(&self) -> &Keys {
        &self.keys
    }

    #[allow(dead_code)]
    pub(crate) fn payloads(&self) -> &Payloads {
        &self.payloads
    }

    /// Upsert a point. Existing ids reuse their slot (overwrite); new ids get
    /// the next free slot (growing capacity when needed).
    pub fn upsert(&self, point: &VectorPoint) -> Result<()> {
        let mut inner = self.inner.write();
        if point.vector.len() != inner.meta.vector_size {
            return Err(VectorSearchError::InvalidVectorDimension {
                expected: inner.meta.vector_size,
                actual: point.vector.len(),
            });
        }
        for (i, v) in point.vector.iter().enumerate() {
            if !v.is_finite() {
                return Err(VectorSearchError::NonFiniteElement(i));
            }
        }

        let slot = if let Some(slot) = inner.reverse.get(&point.id) {
            *slot as u64
        } else {
            let slot = inner.meta.next_slot;
            self.ensure_capacity(&mut inner.meta, slot + 1)?;
            self.keys.append_key(slot as usize, &point.id.to_string())?;
            inner.reverse.insert(point.id.clone(), slot as u32);
            inner.meta.next_slot += 1;
            inner.meta.live_count += 1;
            slot
        };

        self.vectors.write_slot(slot, &point.vector)?;
        self.payloads.append_payload(slot as usize, point.payload.as_ref())?;
        inner.meta.save(&self.dir)?;
        Ok(())
    }

    /// Fetch a point by id.
    pub fn get(&self, id: &PointId) -> Result<Option<VectorPoint>> {
        let inner = self.inner.read();
        let slot = match inner.reverse.get(id) {
            Some(slot) => *slot as u64,
            None => return Ok(None),
        };
        let tombstones = self.tombstones.load();
        if tombstones[slot as usize] {
            return Ok(None);
        }
        let dim = inner.meta.vector_size;
        let vsnap = self.vectors.snapshot();
        let vector = Vectors::read_slot(&vsnap, slot, inner.meta.segment_slots, dim)
            .ok_or_else(|| {
                VectorSearchError::CorruptData(format!("slot {slot} out of vectors.bin range"))
            })?
            .to_vec();
        let payload = Payloads::read_payload(&self.payloads.snapshot(), slot as usize)?;
        Ok(Some(VectorPoint {
            id: id.clone(),
            vector,
            payload,
        }))
    }

    /// Delete a point by id (tombstone; physical removal happens on compaction).
    pub fn delete(&self, id: &PointId) -> Result<bool> {
        let mut inner = self.inner.write();
        let slot = match inner.reverse.remove(id) {
            Some(slot) => slot,
            None => return Ok(false),
        };
        self.payloads.set_tombstone(slot as usize, true)?;
        self.update_tombstone_bit(slot as usize, true);
        inner.meta.live_count = inner.meta.live_count.saturating_sub(1);
        inner.meta.tombstone_count += 1;
        inner.meta.save(&self.dir)?;
        Ok(true)
    }

    /// Number of live points.
    pub fn count(&self) -> u64 {
        self.inner.read().meta.live_count
    }

    /// Grow storage to accommodate `needed_slots` slots (0-indexed high water).
    fn ensure_capacity(&self, meta: &mut Meta, needed_slots: u64) -> Result<()> {
        if needed_slots <= meta.slot_capacity {
            return Ok(());
        }
        self.vectors.grow_to(needed_slots)?;
        let new_capacity = self.vectors.slot_capacity();
        self.keys.grow_to(new_capacity)?;
        self.payloads.grow_to(new_capacity)?;

        let old = self.tombstones.load();
        let mut next = (**old).clone();
        next.resize(new_capacity as usize, false);
        self.tombstones.store(Arc::new(next));

        meta.slot_capacity = new_capacity;
        Ok(())
    }

    fn update_tombstone_bit(&self, slot: usize, value: bool) {
        let old = self.tombstones.load();
        let mut next = (**old).clone();
        next.set(slot, value);
        self.tombstones.store(Arc::new(next));
    }
}

fn validate_collection_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(VectorSearchError::InvalidCollectionName(name.to_string()));
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') || name.contains('\0')
    {
        return Err(VectorSearchError::InvalidCollectionName(name.to_string()));
    }
    Ok(())
}

impl std::fmt::Debug for CollectionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("CollectionStore")
            .field("dir", &self.dir)
            .field("collection", &inner.meta.collection)
            .field("vector_size", &inner.meta.vector_size)
            .field("distance", &inner.meta.distance)
            .field("next_slot", &inner.meta.next_slot)
            .field("live_count", &inner.meta.live_count)
            .field("tombstone_count", &inner.meta.tombstone_count)
            .field("slot_capacity", &inner.meta.slot_capacity)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CollectionConfig, DistanceMetric, VectorPoint};

    fn config(dim: usize) -> CollectionConfig {
        CollectionConfig::new(dim, DistanceMetric::Cosine)
    }

    fn point(id: u64, dim: usize) -> VectorPoint {
        VectorPoint::new(
            id,
            (0..dim).map(|i| ((id as usize * 31 + i) % 100) as f32 / 100.0).collect(),
        )
    }

    fn point_with_payload(id: u64, dim: usize, payload: crate::types::Payload) -> VectorPoint {
        VectorPoint::new(id, (0..dim).map(|_| 0.5).collect()).with_payload(payload)
    }

    #[test]
    fn test_create_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_a");
        let store = CollectionStore::create(&store_dir, "col_a", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        store.upsert(&point(2, 4)).unwrap();
        assert_eq!(store.count(), 2);
        drop(store);

        let reopened = CollectionStore::open(&store_dir).unwrap();
        assert_eq!(reopened.count(), 2);
        let p1 = reopened.get(&PointId::Num(1)).unwrap().unwrap();
        assert_eq!(p1.vector.len(), 4);
        assert_eq!(p1.vector, point(1, 4).vector);
        let p2 = reopened.get(&PointId::Num(2)).unwrap().unwrap();
        assert_eq!(p2.vector, point(2, 4).vector);
        assert!(reopened.get(&PointId::Num(99)).unwrap().is_none());
    }

    #[test]
    fn test_payload_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_p");
        let store = CollectionStore::create(&store_dir, "col_p", &config(2)).unwrap();
        let mut payload = HashMap::new();
        payload.insert("color".to_string(), serde_json::json!("red"));
        payload.insert("size".to_string(), serde_json::json!(42));
        store.upsert(&point_with_payload(1, 2, payload.clone())).unwrap();

        let got = store.get(&PointId::Num(1)).unwrap().unwrap();
        assert_eq!(got.payload, Some(payload.clone()));

        drop(store);
        let reopened = CollectionStore::open(&store_dir).unwrap();
        let got = reopened.get(&PointId::Num(1)).unwrap().unwrap();
        assert_eq!(got.payload, Some(payload));
    }

    #[test]
    fn test_upsert_overwrite_reuses_slot() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_o");
        let store = CollectionStore::create(&store_dir, "col_o", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        store.upsert(&point(2, 4)).unwrap();

        let overwritten = VectorPoint::new(1u64, vec![9.0; 4]);
        store.upsert(&overwritten).unwrap();

        assert_eq!(store.count(), 2, "overwrite must not grow live count");
        let got = store.get(&PointId::Num(1)).unwrap().unwrap();
        assert_eq!(got.vector, vec![9.0; 4]);
        let got2 = store.get(&PointId::Num(2)).unwrap().unwrap();
        assert_eq!(got2.vector, point(2, 4).vector);
    }

    #[test]
    fn test_grow_across_segments() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_g");
        let store =
            CollectionStore::create_with_segment_slots(&store_dir, "col_g", &config(4), 16).unwrap();

        let total = 100u64;
        for i in 0..total {
            store.upsert(&point(i, 4)).unwrap();
        }
        assert_eq!(store.count(), total);
        let meta = store.meta();
        assert!(meta.slot_capacity >= total, "capacity grown to {}", meta.slot_capacity);
        assert_eq!(meta.slot_capacity % meta.segment_slots as u64, 0);

        for i in (0..total).step_by(7) {
            let got = store.get(&PointId::Num(i)).unwrap().unwrap();
            assert_eq!(got.vector, point(i, 4).vector);
        }

        drop(store);
        let reopened = CollectionStore::open(&store_dir).unwrap();
        assert_eq!(reopened.count(), total);
        for i in (0..total).step_by(11) {
            let got = reopened.get(&PointId::Num(i)).unwrap().unwrap();
            assert_eq!(got.vector, point(i, 4).vector);
        }
    }

    #[test]
    fn test_delete_tombstone_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_d");
        let store = CollectionStore::create(&store_dir, "col_d", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        store.upsert(&point(2, 4)).unwrap();
        store.upsert(&point(3, 4)).unwrap();

        assert!(store.delete(&PointId::Num(2)).unwrap());
        assert!(!store.delete(&PointId::Num(2)).unwrap(), "double delete is a no-op");
        assert!(store.get(&PointId::Num(2)).unwrap().is_none());
        assert_eq!(store.count(), 2);
        let meta = store.meta();
        assert_eq!(meta.tombstone_count, 1);

        drop(store);
        let reopened = CollectionStore::open(&store_dir).unwrap();
        assert_eq!(reopened.count(), 2);
        assert!(reopened.get(&PointId::Num(2)).unwrap().is_none());
        assert_eq!(reopened.get(&PointId::Num(1)).unwrap().unwrap().vector, point(1, 4).vector);
        assert_eq!(reopened.get(&PointId::Num(3)).unwrap().unwrap().vector, point(3, 4).vector);
    }

    #[test]
    fn test_upsert_then_delete_then_reinsert() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_r");
        let store = CollectionStore::create(&store_dir, "col_r", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        assert!(store.delete(&PointId::Num(1)).unwrap());

        // Re-insert: allocates a fresh slot (tombstoned slots are not reused
        // until compaction).
        store.upsert(&point(1, 4)).unwrap();
        assert_eq!(store.count(), 1);
        let meta = store.meta();
        assert_eq!(meta.next_slot, 2);
        assert!(store.get(&PointId::Num(1)).unwrap().is_some());
    }

    #[test]
    fn test_rejects_invalid_dimension() {
        let dir = tempfile::tempdir().unwrap();
        let store = CollectionStore::create(dir.path().join("col_v"), "col_v", &config(4)).unwrap();
        let bad = VectorPoint::new(1u64, vec![1.0, 2.0]);
        let err = store.upsert(&bad).unwrap_err();
        assert!(matches!(
            err,
            VectorSearchError::InvalidVectorDimension { expected: 4, actual: 2 }
        ));
    }

    #[test]
    fn test_rejects_non_finite_elements() {
        let dir = tempfile::tempdir().unwrap();
        let store = CollectionStore::create(dir.path().join("col_n"), "col_n", &config(2)).unwrap();
        let bad = VectorPoint::new(1u64, vec![f32::NAN, 1.0]);
        let err = store.upsert(&bad).unwrap_err();
        assert!(matches!(err, VectorSearchError::NonFiniteElement(0)));

        let bad_inf = VectorPoint::new(1u64, vec![1.0, f32::INFINITY]);
        let err = store.upsert(&bad_inf).unwrap_err();
        assert!(matches!(err, VectorSearchError::NonFiniteElement(1)));
    }

    #[test]
    fn test_create_validates_inputs() {
        let dir = tempfile::tempdir().unwrap();

        let err = CollectionStore::create(dir.path().join("bad"), "", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::InvalidCollectionName(_)));

        let err = CollectionStore::create(dir.path().join("bad"), "a/b", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::InvalidCollectionName(_)));

        let mut cfg = config(0);
        cfg.vector_size = 0;
        let err = CollectionStore::create(dir.path().join("bad"), "col", &cfg).unwrap_err();
        assert!(matches!(
            err,
            VectorSearchError::InvalidVectorDimension { expected: 1, actual: 0 }
        ));

        let mut cfg = config(4);
        cfg.distance = DistanceMetric::Manhattan;
        let err = CollectionStore::create(dir.path().join("bad"), "col", &cfg).unwrap_err();
        assert!(matches!(err, VectorSearchError::UnsupportedMetric(DistanceMetric::Manhattan)));
    }

    #[test]
    fn test_create_existing_dir_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_x");
        let _store = CollectionStore::create(&store_dir, "col_x", &config(4)).unwrap();
        let err = CollectionStore::create(&store_dir, "col_x", &config(4)).unwrap_err();
        assert!(matches!(err, VectorSearchError::CollectionAlreadyExists(_)));
    }

    #[test]
    fn test_corrupt_vectors_length_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_c");
        let store = CollectionStore::create(&store_dir, "col_c", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        drop(store);

        // Corrupt vectors.bin by truncating it.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(store_dir.join("vectors.bin"))
            .unwrap();
        file.set_len(16).unwrap();
        drop(file);

        let err = CollectionStore::open(&store_dir).unwrap_err();
        assert!(matches!(err, VectorSearchError::CorruptData(_)));
    }
}