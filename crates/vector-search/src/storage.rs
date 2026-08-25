//! Per-collection storage assembly.
//!
//! A collection directory contains:
//! - `meta.bin`     postcard `Meta`
//! - `vectors.bin`  dense row-major f32, segmented mmap
//! - `keys.bin`     slot -> point id directory
//! - `payloads.bin` slot -> payload blob directory + tombstone flags
//! - `wal.bin`      append-only transaction log (crash recovery)
//!
//! Writes are serialized through the store's `RwLock`; readers snapshot the
//! mmap-backed `ArcSwap` views and can scan without the lock afterwards.
//! Mutations go through [`CollectionStore::apply_txn`]: the WAL is appended
//! and fsync'ed before the transaction is applied to memory, so a crash at
//! any point recovers by idempotent replay.
//!
//! This module hosts the store type itself plus its lifecycle and
//! mutation/read paths; orthogonal concerns live in sibling modules:
//! - [`tombstones`]   lock-free tombstone table
//! - [`compaction`]   physical tombstone reclamation
//! - [`index_lifecycle`]  ANN index load/build/publish/drop
//! - [`search`]       exact scan and ANN search execution

mod compaction;
mod directory;
mod index_lifecycle;
mod keys;
mod meta;
mod payloads;
mod search;
mod tombstones;
pub(crate) mod vectors;
mod wal;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bitvec::prelude::*;
use parking_lot::{Mutex, RwLock};

pub(crate) use meta::Meta;
pub(crate) use tombstones::TombstoneBits;
pub use wal::{Wal, WalPoint, WalRecord, WalTxn};

use crate::error::{Result, VectorSearchError};
use crate::types::{CollectionConfig, DistanceMetric, IndexType, IvfConfig, PointId, VectorPoint};

use self::index_lifecycle::PublishedIndex;
use self::keys::Keys;
use self::payloads::Payloads;
use self::vectors::Vectors;

/// Tombstone ratio above which deletes trigger compaction.
const COMPACTION_THRESHOLD: f64 = 0.20;

/// In-memory mutable state, guarded by the store's `RwLock`.
struct StoreInner {
    meta: Meta,
    reverse: HashMap<PointId, u32>,
    /// IVF configuration for `IndexType::IVF` collections. Persisted in
    /// `meta.bin`; the engine may still override it at runtime via
    /// [`CollectionStore::set_ivf_config`].
    ivf_config: Option<IvfConfig>,
    /// Last measured drift ratio, exposed via `CollectionInfo`.
    last_drift_ratio: Option<f64>,
}

/// A single opened collection.
pub struct CollectionStore {
    dir: PathBuf,
    inner: RwLock<StoreInner>,
    /// Tombstone mirror (slot -> deleted) for lock-free scans.
    tombstones: ArcSwap<TombstoneBits>,
    vectors: Vectors,
    keys: Keys,
    payloads: Payloads,
    wal: Wal,
    /// Published ANN index; `None` = exact scan. Swapped atomically.
    index: ArcSwap<Option<PublishedIndex>>,
    /// Slots inserted while no index was published and a build was in
    /// flight; drained into the index on publish so probe search never
    /// misses them.
    pending: RwLock<Vec<u32>>,
    /// Serializes compaction vs index build/rebuild. Lock order:
    /// maintenance -> inner.write; never taken while holding inner.write.
    maintenance: Mutex<()>,
    /// Set while an index build is in flight (routes inserts to `pending`).
    building: AtomicBool,
    /// Compaction invalidated a published index and a rebuild should be
    /// scheduled by the engine maintenance worker.
    needs_rebuild: AtomicBool,
    /// Monotonic counter bumped on every applied mutation batch. Compaction
    /// uses it to detect that no write raced its temp-file rewrite phase, so
    /// the commit can swap the files without re-validating every slot.
    mutations: AtomicU64,
}

impl CollectionStore {
    /// Create a new collection directory and open it.
    pub fn create(
        dir: impl AsRef<Path>,
        collection: &str,
        config: &CollectionConfig,
    ) -> Result<Self> {
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
        if matches!(
            config.index_type.unwrap_or(IndexType::HNSW),
            IndexType::HNSW
        ) {
            if let Some(hnsw) = &config.hnsw_config {
                hnsw.validate()?;
            }
        }

        let dir = dir.as_ref();
        if dir.exists() {
            return Err(VectorSearchError::CollectionAlreadyExists(
                collection.to_string(),
            ));
        }
        std::fs::create_dir_all(dir)?;

        let mut meta = Meta::new_with_segment_slots(
            collection,
            config.vector_size,
            config.distance,
            segment_slots,
        );
        // HNSW is the default ANN tier; FLAT keeps exact scan only.
        meta.index_type = config.index_type.unwrap_or(IndexType::HNSW);
        match meta.index_type {
            IndexType::HNSW => {
                meta.hnsw_config = Some(config.hnsw_config.clone().unwrap_or_default());
                meta.ivf_config = None;
            }
            IndexType::IVF => {
                meta.ivf_config = config.ivf_config.clone();
                meta.hnsw_config = None;
            }
            IndexType::FLAT => {
                meta.ivf_config = None;
                meta.hnsw_config = None;
            }
        }
        meta.save(dir)?;

        let vectors = Vectors::create(
            &dir.join("vectors.bin"),
            config.vector_size,
            meta.segment_slots,
        )?;
        let keys = Keys::create(&dir.join("keys.bin"), meta.slot_capacity)?;
        let payloads = Payloads::create(&dir.join("payloads.bin"), meta.slot_capacity)?;

        let tombstones = ArcSwap::from(Arc::new(TombstoneBits::new(meta.slot_capacity as usize)));
        let wal = Wal::open_or_create(&dir.join("wal.bin"))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            inner: RwLock::new(StoreInner {
                meta,
                reverse: HashMap::new(),
                ivf_config: config.ivf_config.clone(),
                last_drift_ratio: None,
            }),
            tombstones,
            vectors,
            keys,
            payloads,
            wal,
            index: ArcSwap::from(Arc::new(None)),
            pending: RwLock::new(Vec::new()),
            maintenance: Mutex::new(()),
            building: AtomicBool::new(false),
            needs_rebuild: AtomicBool::new(false),
            mutations: AtomicU64::new(0),
        })
    }

    /// Open an existing collection directory, rebuilding the id->slot map and
    /// replaying the WAL (idempotent).
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let meta = Meta::load(dir)?;

        let vectors = Vectors::open(
            &dir.join("vectors.bin"),
            meta.vector_size,
            meta.segment_slots,
            meta.slot_capacity,
        )?;
        let keys = Keys::open(&dir.join("keys.bin"))?;
        let payloads = Payloads::open(&dir.join("payloads.bin"))?;

        let mut reverse = HashMap::new();
        let mut tombstones = bitvec![0; meta.slot_capacity as usize];
        {
            let keys_view = keys.snapshot();
            let payloads_view = payloads.snapshot();
            for slot in 0..meta.next_slot as usize {
                if Payloads::is_tombstoned(&payloads_view, slot) {
                    tombstones.set(slot, true);
                    continue;
                }
                if let Some(key) = Keys::read_key(&keys_view, slot)? {
                    let id = PointId::from(key);
                    reverse.insert(id, slot as u32);
                }
            }
        }

        let store = Self {
            dir: dir.to_path_buf(),
            inner: RwLock::new(StoreInner {
                ivf_config: meta.ivf_config.clone(),
                meta,
                reverse,
                last_drift_ratio: None,
            }),
            tombstones: ArcSwap::from(Arc::new(TombstoneBits::from_bits(tombstones))),
            vectors,
            keys,
            payloads,
            wal: Wal::open_or_create(&dir.join("wal.bin"))?,
            index: ArcSwap::from(Arc::new(None)),
            pending: RwLock::new(Vec::new()),
            maintenance: Mutex::new(()),
            building: AtomicBool::new(false),
            needs_rebuild: AtomicBool::new(false),
            mutations: AtomicU64::new(0),
        };
        store.replay_wal()?;
        store.load_index()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Snapshot of the metadata (dimension, metric, counts).
    pub fn meta(&self) -> Meta {
        self.inner.read().meta.clone()
    }

    /// Upsert a point. Existing ids reuse their slot (overwrite); new ids get
    /// the next free slot (growing capacity when needed).
    ///
    /// Raw apply without WAL: engine paths must use [`CollectionStore::apply_txn`]
    /// so that mutations are crash-safe and recoverable by replay.
    pub fn upsert(&self, point: &VectorPoint) -> Result<()> {
        let mut inner = self.inner.write();
        self.apply_upsert_locked(&mut inner, point)?;
        inner.meta.save(&self.dir)?;
        Ok(())
    }

    /// Delete a point by id (tombstone; physical removal happens on
    /// compaction). Returns whether the point existed.
    pub fn delete(&self, id: &PointId) -> Result<bool> {
        let mut inner = self.inner.write();
        let slot = match inner.reverse.remove(id) {
            Some(slot) => slot,
            None => return Ok(false),
        };
        self.apply_delete_slot_locked(&mut inner, slot)?;
        // Physical reclamation is scheduled by the engine's maintenance
        // worker once the tombstone ratio crosses the compaction threshold;
        // reads already filter tombstoned slots.
        Ok(true)
    }

    /// Apply a WAL-backed transaction:
    ///
    /// 1. validate every op (no invalid data may be logged);
    /// 2. append the whole `WalTxn` to `wal.bin` + fsync;
    /// 3. apply to memory and the mmap files;
    /// 4. persist `meta.bin` (with `last_applied_txn` advanced).
    ///
    /// On success the caller's buffer may be drained; on failure nothing was
    /// applied in memory and the call may be retried (replay is idempotent).
    pub fn apply_txn(&self, txn: &WalTxn) -> Result<()> {
        {
            let mut inner = self.inner.write();
            for op in &txn.ops {
                if let WalRecord::Upsert { point } = op {
                    let point = point.to_point()?;
                    validate_point(&inner.meta, &point)?;
                }
            }
            self.wal.append(txn)?;
            self.apply_records_locked(&mut inner, &txn.ops)?;
            // Monotonic water mark: late/duplicated txn ids must not regress
            // the last applied id (replay is idempotent, so this is safe).
            inner.meta.last_applied_txn = inner.meta.last_applied_txn.max(txn.txn_id);
            inner.meta.save(&self.dir)?;
        }
        Ok(())
    }

    /// Apply a WAL-backed batch of records with an auto-assigned txn id.
    ///
    /// This is the crash-safe path for single-collection operations exposed by
    /// the engine; unlike [`CollectionStore::apply_txn`] the caller does not
    /// coordinate a graph transaction id.
    pub fn apply_ops(&self, ops: &[WalRecord]) -> Result<()> {
        let txn_id = self.inner.read().meta.last_applied_txn + 1;
        self.apply_txn(&WalTxn {
            txn_id,
            ops: ops.to_vec(),
        })
    }

    /// Delete every live point matching `filter`. Returns the number of
    /// deleted points. Runs as a single WAL-backed batch so recovery replays
    /// the filter match deterministically.
    pub fn delete_by_filter(&self, filter: &crate::types::VectorFilter) -> Result<u64> {
        let point_ids: Vec<String> = {
            let inner = self.inner.read();
            let tombstones = self.tombstones.load();
            let keysnap = self.keys.snapshot();
            let psnap = self.payloads.snapshot();
            let mut point_ids = Vec::new();
            for slot in 0..inner.meta.next_slot as usize {
                if tombstones.bit(slot) {
                    continue;
                }
                let key = Keys::read_key(&keysnap, slot)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("slot {slot} has no key"))
                })?;
                let id = PointId::from(key);
                let payload = Payloads::read_payload(&psnap, slot)?;
                if crate::filter::matches(filter, &id, payload.as_ref())? {
                    point_ids.push(id.to_string());
                }
            }
            point_ids
        };
        if point_ids.is_empty() {
            return Ok(0);
        }
        let deleted = point_ids.len() as u64;
        self.apply_ops(&[WalRecord::DeleteBatch { point_ids }])?;
        Ok(deleted)
    }

    /// Replay the WAL into memory. Idempotent: upserts overwrite by point id,
    /// deletes of missing points are no-ops, `Compact` only advances the
    /// water mark. Counts are reconciled against the actual slot state, which
    /// covers the crash window where the WAL was fsync'ed but `meta.bin` was
    /// not yet written.
    fn replay_wal(&self) -> Result<()> {
        let last = self.wal.replay(|txn| {
            let mut inner = self.inner.write();
            self.apply_records_locked(&mut inner, &txn.ops)?;
            Ok(())
        })?;
        let mut inner = self.inner.write();
        inner.meta.last_applied_txn = inner.meta.last_applied_txn.max(last);
        let live = inner.reverse.len() as u64;
        let tomb = self.tombstones.load().count_ones();
        let changed = inner.meta.live_count != live || inner.meta.tombstone_count != tomb;
        inner.meta.live_count = live;
        inner.meta.tombstone_count = tomb;
        if changed {
            inner.meta.save(&self.dir)?;
        }
        Ok(())
    }

    /// WAL accessor for the engine (per-collection transactions) and tests.
    pub fn wal(&self) -> &Wal {
        &self.wal
    }

    fn apply_records_locked(&self, inner: &mut StoreInner, ops: &[WalRecord]) -> Result<()> {
        // One bump per batch: compaction compares this counter across its
        // temp-file rewrite phase to prove no write raced it.
        self.mutations.fetch_add(1, AtomicOrdering::Relaxed);
        for op in ops {
            match op {
                WalRecord::Upsert { point } => {
                    let point = point.to_point()?;
                    self.apply_upsert_locked(inner, &point)?;
                }
                WalRecord::Delete { point_id } => self.apply_delete_locked(inner, point_id)?,
                WalRecord::DeleteBatch { point_ids } => {
                    for id in point_ids {
                        self.apply_delete_locked(inner, id)?;
                    }
                }
                // Checkpoint markers carry no data.
                WalRecord::Compact | WalRecord::DropCollection => {}
            }
        }
        Ok(())
    }

    fn apply_upsert_locked(&self, inner: &mut StoreInner, point: &VectorPoint) -> Result<()> {
        validate_point(&inner.meta, point)?;

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
        self.payloads
            .append_payload(slot as usize, point.payload.as_ref())?;
        self.register_slot(slot as u32, &point.vector, inner.meta.segment_slots);
        Ok(())
    }

    fn apply_delete_locked(&self, inner: &mut StoreInner, point_id: &str) -> Result<()> {
        let id = PointId::from(point_id.to_string());
        if let Some(slot) = inner.reverse.remove(&id) {
            self.apply_delete_slot_locked(inner, slot)?;
        }
        Ok(())
    }

    fn apply_delete_slot_locked(&self, inner: &mut StoreInner, slot: u32) -> Result<()> {
        self.payloads.set_tombstone(slot as usize, true)?;
        self.update_tombstone_bit(slot as usize, true);
        inner.meta.live_count = inner.meta.live_count.saturating_sub(1);
        inner.meta.tombstone_count += 1;
        Ok(())
    }

    /// Fetch a point by id.
    pub fn get(&self, id: &PointId) -> Result<Option<VectorPoint>> {
        let inner = self.inner.read();
        let slot = match inner.reverse.get(id) {
            Some(slot) => *slot as u64,
            None => return Ok(None),
        };
        if self.tombstones.load().bit(slot as usize) {
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

    /// Number of live points.
    pub fn count(&self) -> u64 {
        self.inner.read().meta.live_count
    }

    /// Whether the tombstone ratio has crossed the compaction threshold.
    ///
    /// The engine polls this after mutations and schedules a background
    /// compaction; visibility of data never depends on compaction having run.
    pub fn needs_compaction(&self) -> bool {
        let inner = self.inner.read();
        threshold_met(&inner.meta)
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

        let next = self.tombstones.load().resized(new_capacity as usize);
        self.tombstones.store(Arc::new(next));

        meta.slot_capacity = new_capacity;
        Ok(())
    }

    fn update_tombstone_bit(&self, slot: usize, value: bool) {
        let next = self.tombstones.load().with_slot(slot, value);
        self.tombstones.store(Arc::new(next));
    }
}

fn validate_point(meta: &Meta, point: &VectorPoint) -> Result<()> {
    if point.vector.len() != meta.vector_size {
        return Err(VectorSearchError::InvalidVectorDimension {
            expected: meta.vector_size,
            actual: point.vector.len(),
        });
    }
    for (i, v) in point.vector.iter().enumerate() {
        if !v.is_finite() {
            return Err(VectorSearchError::NonFiniteElement(i));
        }
    }
    Ok(())
}

fn threshold_met(meta: &Meta) -> bool {
    meta.next_slot > 0 && meta.tombstone_count as f64 / meta.next_slot as f64 > COMPACTION_THRESHOLD
}

pub(crate) fn validate_collection_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(VectorSearchError::InvalidCollectionName(name.to_string()));
    }
    if name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
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
            (0..dim)
                .map(|i| ((id as usize * 31 + i) % 100) as f32 / 100.0)
                .collect(),
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
        store
            .upsert(&point_with_payload(1, 2, payload.clone()))
            .unwrap();

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
        let store = CollectionStore::create_with_segment_slots(&store_dir, "col_g", &config(4), 16)
            .unwrap();

        let total = 100u64;
        for i in 0..total {
            store.upsert(&point(i, 4)).unwrap();
        }
        assert_eq!(store.count(), total);
        let meta = store.meta();
        assert!(
            meta.slot_capacity >= total,
            "capacity grown to {}",
            meta.slot_capacity
        );
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
        assert!(
            !store.delete(&PointId::Num(2)).unwrap(),
            "double delete is a no-op"
        );
        assert!(store.get(&PointId::Num(2)).unwrap().is_none());
        assert_eq!(store.count(), 2);
        // The tombstone stays pending until compaction runs (the engine
        // schedules it in the background); visibility never depends on it.
        let meta = store.meta();
        assert_eq!(meta.tombstone_count, 1);
        store.compact().unwrap();
        let meta = store.meta();
        assert_eq!(meta.tombstone_count, 0);
        assert_eq!(meta.next_slot, 2);

        drop(store);
        let reopened = CollectionStore::open(&store_dir).unwrap();
        assert_eq!(reopened.count(), 2);
        assert!(reopened.get(&PointId::Num(2)).unwrap().is_none());
        assert_eq!(
            reopened.get(&PointId::Num(1)).unwrap().unwrap().vector,
            point(1, 4).vector
        );
        assert_eq!(
            reopened.get(&PointId::Num(3)).unwrap().unwrap().vector,
            point(3, 4).vector
        );
    }

    #[test]
    fn test_upsert_then_delete_then_reinsert() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("col_r");
        let store = CollectionStore::create(&store_dir, "col_r", &config(4)).unwrap();
        store.upsert(&point(1, 4)).unwrap();
        assert!(store.delete(&PointId::Num(1)).unwrap());

        // Without an intervening compaction the re-insert takes a fresh slot:
        // the stale tombstone stays pending and flags the collection for
        // background compaction, while visibility is already correct.
        store.upsert(&point(1, 4)).unwrap();
        assert_eq!(store.count(), 1);
        let meta = store.meta();
        assert_eq!(meta.next_slot, 2);
        assert_eq!(meta.tombstone_count, 1);
        assert!(store.needs_compaction());
        store.compact().unwrap();
        assert!(!store.needs_compaction());
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
            VectorSearchError::InvalidVectorDimension {
                expected: 4,
                actual: 2
            }
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
            VectorSearchError::InvalidVectorDimension {
                expected: 1,
                actual: 0
            }
        ));

        let mut cfg = config(4);
        cfg.distance = DistanceMetric::Manhattan;
        let err = CollectionStore::create(dir.path().join("bad"), "col", &cfg).unwrap_err();
        assert!(matches!(
            err,
            VectorSearchError::UnsupportedMetric(DistanceMetric::Manhattan)
        ));
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
