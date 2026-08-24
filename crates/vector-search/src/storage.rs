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

mod compaction;
mod directory;
mod keys;
mod meta;
mod payloads;
pub(crate) mod vectors;
mod wal;

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bitvec::prelude::*;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;

pub(crate) use meta::Meta;
pub use wal::{Wal, WalPoint, WalRecord, WalTxn};

use crate::error::{Result, VectorSearchError};
use crate::index::{persist, IvfIndex};
use crate::types::{
    CollectionConfig, DistanceMetric, IndexInfo, IvfConfig, PointId, SearchQuery, SearchResult,
    VectorPoint,
};

use self::compaction::{plan_slots, write_dir_file, write_vectors_file, DirEntry};
use self::directory::{DirView, KEY_REC_SIZE, SLOT_REC_SIZE};
use self::keys::Keys;
use self::payloads::Payloads;
use self::vectors::Vectors;

/// Tombstone ratio above which deletes trigger compaction.
const COMPACTION_THRESHOLD: f64 = 0.20;

/// Bits per tombstone chunk (8 KiB of bitmap).
const TOMBSTONE_CHUNK_BITS: usize = 1 << 16;

/// Lock-free tombstone table held as fixed-size immutable chunks.
///
/// Setting one bit clones a single 8 KiB chunk instead of the whole bitmap,
/// whose size grows with collection capacity; readers snapshot the chunk list
/// through an [`ArcSwap`] exactly as before, so scan cost is unchanged.
#[derive(Debug, Default)]
pub(crate) struct TombstoneBits {
    chunks: Vec<Arc<BitVec>>,
}

impl TombstoneBits {
    fn new(slot_capacity: usize) -> Self {
        let chunk_count = slot_capacity.div_ceil(TOMBSTONE_CHUNK_BITS).max(1);
        Self {
            chunks: (0..chunk_count)
                .map(|_| Arc::new(bitvec::bitvec![0; TOMBSTONE_CHUNK_BITS]))
                .collect(),
        }
    }

    pub(crate) fn from_bits(bits: BitVec) -> Self {
        let mut chunks = Vec::new();
        for chunk in bits.chunks(TOMBSTONE_CHUNK_BITS) {
            chunks.push(Arc::new(chunk.to_bitvec()));
        }
        if chunks.is_empty() {
            chunks.push(Arc::new(bitvec::bitvec![0; TOMBSTONE_CHUNK_BITS]));
        }
        Self { chunks }
    }

    pub(crate) fn bit(&self, slot: usize) -> bool {
        let (chunk, offset) = (slot / TOMBSTONE_CHUNK_BITS, slot % TOMBSTONE_CHUNK_BITS);
        match self.chunks.get(chunk) {
            Some(c) => c.as_bitslice()[offset],
            None => false,
        }
    }

    fn count_ones(&self) -> u64 {
        self.chunks
            .iter()
            .map(|c| c.as_bitslice().count_ones() as u64)
            .sum()
    }

    /// Copy-on-write single-bit update: only the affected chunk is cloned.
    fn with_slot(&self, slot: usize, value: bool) -> Self {
        let (chunk, offset) = (slot / TOMBSTONE_CHUNK_BITS, slot % TOMBSTONE_CHUNK_BITS);
        let mut next = Self {
            chunks: Vec::with_capacity(self.chunks.len()),
        };
        for (index, existing) in self.chunks.iter().enumerate() {
            if index == chunk {
                let mut copy = (**existing).clone();
                if offset < copy.len() {
                    copy.set(offset, value);
                }
                next.chunks.push(Arc::new(copy));
            } else {
                next.chunks.push(Arc::clone(existing));
            }
        }
        next
    }

    /// Grow or shrink to `slot_capacity` slots, preserving existing bits.
    fn resized(&self, slot_capacity: usize) -> Self {
        let mut bits = BitVec::with_capacity(slot_capacity);
        for chunk in &self.chunks {
            bits.extend_from_bitslice(chunk.as_bitslice());
        }
        bits.resize(slot_capacity, false);
        Self::from_bits(bits)
    }
}

/// In-memory mutable state, guarded by the store's `RwLock`.
struct StoreInner {
    meta: Meta,
    reverse: HashMap<PointId, u32>,
    /// IVF configuration. Persisted in `meta.bin` since format v2; the
    /// engine may still override it at runtime via [`CollectionStore::set_ivf_config`].
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
    /// Published IVF index; `None` = exact scan. Swapped atomically.
    ivf: ArcSwap<Option<Arc<IvfIndex>>>,
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
}

/// Snapshot of the immutable views used by a single search pass.
struct SearchSnapshot<'a> {
    dim: usize,
    segment_slots: u32,
    tombstones: &'a TombstoneBits,
    vsnap: &'a [Arc<memmap2::Mmap>],
    keysnap: &'a DirView,
    paysnap: &'a DirView,
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
        meta.ivf_config = config.ivf_config.clone();
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
            ivf: ArcSwap::from(Arc::new(None)),
            pending: RwLock::new(Vec::new()),
            maintenance: Mutex::new(()),
            building: AtomicBool::new(false),
            needs_rebuild: AtomicBool::new(false),
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
            ivf: ArcSwap::from(Arc::new(None)),
            pending: RwLock::new(Vec::new()),
            maintenance: Mutex::new(()),
            building: AtomicBool::new(false),
            needs_rebuild: AtomicBool::new(false),
        };
        store.replay_wal()?;
        store.load_ivf()?;
        Ok(store)
    }

    /// Supply the IVF configuration after open. Collections created since
    /// format v2 persist their effective config, but the engine-provided
    /// runtime configuration takes precedence. Applies both to future builds
    /// and to any published index, so a rehydrated index immediately honors
    /// the runtime config.
    pub fn set_ivf_config(&self, config: IvfConfig) {
        self.inner.write().ivf_config = Some(config.clone());
        if let Some(index) = self.ivf.load().as_ref() {
            index.set_config(config);
        }
    }
    /// Rehydrate a persisted IVF index if present and consistent with the
    /// live metadata; otherwise fall back to exact scan. Slots appended after
    /// the build (WAL replay) land in `pending` so probe search still sees
    /// them.
    fn load_ivf(&self) -> Result<()> {
        let Some(data) = persist::load(&self.dir)? else {
            return Ok(());
        };
        let (dim, distance, next_slot, capacity) = {
            let inner = self.inner.read();
            (
                inner.meta.vector_size,
                inner.meta.distance,
                inner.meta.next_slot,
                inner.meta.slot_capacity as usize,
            )
        };
        if !data.valid_for(dim, distance, next_slot) {
            tracing::info!("vector index.bin inconsistent with meta; falling back to exact scan");
            return Ok(());
        }
        let config = self.inner.read().ivf_config.clone().unwrap_or_default();
        let covered = data.slot_list.len();
        let index = Arc::new(IvfIndex::from_persisted(data, config));
        {
            let tombstones = self.tombstones.load();
            let mut pending = self.pending.write();
            for slot in covered..next_slot as usize {
                if slot < capacity && !tombstones.bit(slot) {
                    pending.push(slot as u32);
                }
            }
        }
        self.ivf.store(Arc::new(Some(index)));
        Ok(())
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
        self.register_slot(slot as u32, &point.vector);
        Ok(())
    }

    /// Route a freshly written slot into the published index (nearest list)
    /// or, when no index is published but one is being built, into the
    /// pending set so probe search never misses it.
    fn register_slot(&self, slot: u32, vector: &[f32]) {
        let ivf = self.ivf.load();
        if let Some(index) = ivf.as_ref() {
            index.assign_slot(slot, vector);
            index.note_upsert();
            return;
        }
        drop(ivf);
        if self.building.load(AtomicOrdering::Relaxed) {
            self.pending.write().push(slot);
        }
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

    /// Physically remove tombstoned slots and rebuild all files with compacted
    /// slot numbering `0..live_count`.
    ///
    /// Runs under the store's write lock (blocking searches, acceptable for a
    /// single-node deployment) and holds the `maintenance` mutex, so an index
    /// build cannot run concurrently and observe torn slot numbers.
    /// Procedure:
    /// 1. write `vectors_tmp.bin`/`keys_tmp.bin`/`payloads_tmp.bin`;
    /// 2. fsync each and rename over the live file (atomic swap);
    /// 3. rebuild mmap snapshots, `reverse` map and tombstone bitmap;
    /// 4. rewrite `meta.bin`;
    /// 5. drop any published IVF index (slot numbers changed wholesale) and
    ///    flag the engine maintenance worker for a rebuild;
    /// 6. append a `Compact` checkpoint to the WAL and truncate it.
    ///
    /// Returns the number of live points after compaction.
    pub fn compact(&self) -> Result<u64> {
        let _guard = self.maintenance.lock();
        let had_index = self.ivf.load().is_some();
        let mut inner = self.inner.write();
        if inner.meta.tombstone_count == 0 || inner.meta.next_slot == 0 {
            return Ok(inner.meta.live_count);
        }
        let dim = inner.meta.vector_size;
        let segment_slots = inner.meta.segment_slots;

        let tombstones = self.tombstones.load();
        let (new_capacity, map) =
            plan_slots(|s| !tombstones.bit(s), inner.meta.next_slot, segment_slots);
        let live_count = map.iter().filter(|s| **s != u32::MAX).count() as u64;
        drop(tombstones);

        // 1. vectors.bin
        let tmp_vectors = self.dir.join("vectors_tmp.bin");
        {
            let vsnap = self.vectors.snapshot();
            write_vectors_file(&tmp_vectors, dim, segment_slots, new_capacity, &vsnap, &map)?;
        }
        self.vectors.replace_from(&tmp_vectors)?;

        // 2. keys.bin
        let tmp_keys = self.dir.join("keys_tmp.bin");
        {
            let keys_view = self.keys.snapshot();
            let mut entries = Vec::with_capacity(live_count as usize);
            for (old_slot, new_slot) in map.iter().enumerate() {
                if *new_slot == u32::MAX {
                    continue;
                }
                let key = Keys::read_key(&keys_view, old_slot)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("live slot {old_slot} has no key"))
                })?;
                entries.push(DirEntry {
                    slot: *new_slot,
                    blob: key.into_bytes(),
                    flags: 0,
                });
            }
            write_dir_file(&tmp_keys, *b"VKEY", KEY_REC_SIZE, new_capacity, &entries)?;
        }
        self.keys.replace_from(&tmp_keys)?;

        // 3. payloads.bin
        let tmp_payloads = self.dir.join("payloads_tmp.bin");
        {
            let payloads_view = self.payloads.snapshot();
            let mut entries = Vec::with_capacity(live_count as usize);
            for (old_slot, new_slot) in map.iter().enumerate() {
                if *new_slot == u32::MAX {
                    continue;
                }
                let blob = match Payloads::read_payload(&payloads_view, old_slot)? {
                    Some(p) => serde_json::to_vec(&p)?,
                    None => Vec::new(),
                };
                entries.push(DirEntry {
                    slot: *new_slot,
                    blob,
                    flags: 0,
                });
            }
            write_dir_file(
                &tmp_payloads,
                *b"VPLD",
                SLOT_REC_SIZE,
                new_capacity,
                &entries,
            )?;
        }
        self.payloads.replace_from(&tmp_payloads)?;

        // 4. in-memory rebuild + meta.bin
        self.tombstones
            .store(Arc::new(TombstoneBits::new(new_capacity as usize)));
        inner.reverse.clear();
        {
            let keys_view = self.keys.snapshot();
            for slot in 0..live_count as usize {
                let key = Keys::read_key(&keys_view, slot)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("live slot {slot} has no key"))
                })?;
                inner.reverse.insert(PointId::from(key), slot as u32);
            }
        }
        inner.meta.slot_capacity = new_capacity;
        inner.meta.next_slot = live_count;
        inner.meta.live_count = live_count;
        inner.meta.tombstone_count = 0;
        inner.meta.save(&self.dir)?;

        // 5. Invalidate the IVF index: slot numbering changed wholesale.
        self.ivf.store(Arc::new(None));
        self.pending.write().clear();
        self.building.store(false, AtomicOrdering::Relaxed);
        if had_index {
            self.needs_rebuild.store(true, AtomicOrdering::Relaxed);
        }
        persist::discard(&self.dir.join("index.bin"));

        // 6. WAL checkpoint + truncate
        self.wal.append(&WalTxn {
            txn_id: inner.meta.last_applied_txn,
            ops: vec![WalRecord::Compact],
        })?;
        self.wal.truncate()?;

        Ok(live_count)
    }

    // ---- IVF index lifecycle ----

    /// Build and publish an IVF index from the current live set.
    ///
    /// Heavy work (sampling + k-means) runs off the store lock under the
    /// `maintenance` mutex, so compaction cannot interleave and slot numbers
    /// stay stable. Publication happens atomically under the store write
    /// lock: slots appended during the build are adopted, pending slots are
    /// drained, and only afterwards does the index become visible, so probe
    /// search never misses a point.
    ///
    /// Returns whether a usable index is now published.
    pub fn build_index(&self) -> Result<bool> {
        let Some(config) = self.inner.read().ivf_config.clone() else {
            return Ok(false);
        };
        let _guard = self.maintenance.lock();

        let (dim, metric, segment_slots, snapshot_next_slot, name) = {
            let inner = self.inner.read();
            (
                inner.meta.vector_size,
                inner.meta.distance,
                inner.meta.segment_slots,
                inner.meta.next_slot,
                inner.meta.collection.clone(),
            )
        };
        let live: Vec<u32> = {
            let tombstones = self.tombstones.load();
            (0..snapshot_next_slot as u32)
                .filter(|&s| !tombstones.bit(s as usize))
                .collect()
        };
        if (live.len() as u64) < config.min_build_points.max(1) || live.is_empty() {
            return Ok(false);
        }

        self.building.store(true, AtomicOrdering::Relaxed);
        tracing::info!(
            collection = %name,
            points = live.len(),
            "building IVF index"
        );

        let built = {
            let vsnap = self.vectors.snapshot();
            IvfIndex::build(&config, &name, dim, metric, &live, &vsnap, segment_slots).map(Arc::new)
        };

        let index = match built {
            Ok(index) => index,
            Err(e) => {
                self.building.store(false, AtomicOrdering::Relaxed);
                tracing::warn!(collection = %name, error = %e, "IVF build failed");
                return Err(e);
            }
        };

        // Publish atomically with respect to writers: everything below holds
        // the store write lock (acquired before the pending lock, matching
        // `register_slot`'s order) so concurrent upserts either ran before
        // (and their slots are adopted/drained here) or run after (and see
        // the published index directly).
        let persisted = {
            let inner = self.inner.write();
            let mut pending = self.pending.write();
            let tombstones = self.tombstones.load();
            let vsnap = self.vectors.snapshot();
            index.adopt_range(
                snapshot_next_slot as u32,
                inner.meta.next_slot as u32,
                &tombstones,
                &vsnap,
                segment_slots,
            );
            for slot in pending.drain(..) {
                if !tombstones.bit(slot as usize) {
                    if let Some(v) = Vectors::read_slot(&vsnap, slot as u64, segment_slots, dim) {
                        index.assign_slot(slot, v);
                    }
                }
            }
            index.to_persisted()
        };
        // Publish before clearing the building flag: an upsert that loaded
        // `ivf == None` and then observed `building == false` would record its
        // slot nowhere and go missing from probe searches. With this order
        // such an upsert still sees `building == true` and lands in `pending`,
        // which every probe search scans unconditionally.
        self.ivf.store(Arc::new(Some(index)));
        self.building.store(false, AtomicOrdering::Relaxed);
        self.needs_rebuild.store(false, AtomicOrdering::Relaxed);

        if let Err(e) = persist::save(&self.dir, &persisted) {
            tracing::warn!(collection = %name, error = %e, "index.bin save failed");
        }
        tracing::info!(
            collection = %name,
            lists = persisted.lists,
            "IVF index published"
        );
        Ok(true)
    }

    /// Drop the published index and return to exact scan.
    pub fn drop_index(&self) -> Result<()> {
        let _guard = self.maintenance.lock();
        self.ivf.store(Arc::new(None));
        self.pending.write().clear();
        self.building.store(false, AtomicOrdering::Relaxed);
        self.needs_rebuild.store(false, AtomicOrdering::Relaxed);
        persist::discard(&self.dir.join("index.bin"));
        Ok(())
    }

    /// Whether an index is published.
    pub fn has_index(&self) -> bool {
        self.ivf.load().is_some()
    }

    /// Swap-and-reset the post-compaction rebuild flag.
    pub(crate) fn take_needs_rebuild(&self) -> bool {
        self.needs_rebuild.swap(false, AtomicOrdering::Relaxed)
    }

    pub(crate) fn ivf_config_opt(&self) -> Option<IvfConfig> {
        self.inner.read().ivf_config.clone()
    }

    /// Measure the current drift ratio of the published index.
    pub(crate) fn measure_drift(&self, index: &IvfIndex) -> f64 {
        let (next_slot, segment_slots) = {
            let inner = self.inner.read();
            (inner.meta.next_slot as u32, inner.meta.segment_slots)
        };
        let samples = IvfIndex::sample_plan(index.config().sample_limit, next_slot);
        if samples.is_empty() {
            return 0.0;
        }
        let tombstones = self.tombstones.load();
        let vsnap = self.vectors.snapshot();
        index.drift_ratio(&samples, &tombstones, &vsnap, segment_slots)
    }

    /// Published index plus its configuration, for the maintenance worker.
    pub(crate) fn ivf_state(&self) -> Option<(Arc<IvfIndex>, IvfConfig)> {
        let published: Option<Arc<IvfIndex>> = self.ivf.load().as_ref().clone();
        let index = published?;
        let config = self.inner.read().ivf_config.clone()?;
        Some((index, config))
    }

    pub(crate) fn record_drift(&self, ratio: f64) {
        self.inner.write().last_drift_ratio = Some(ratio);
    }

    /// Current index state for [`CollectionInfo`](crate::types::CollectionInfo).
    pub fn index_info(&self) -> Option<IndexInfo> {
        let ivf = self.ivf.load();
        match ivf.as_ref() {
            Some(index) => {
                let info = IndexInfo {
                    index_kind: 1,
                    lists: index.list_count() as u32,
                    nprobe_default: index.default_nprobe(),
                    built_at_live_count: index.built_at_live_count(),
                    last_drift_ratio: self.inner.read().last_drift_ratio,
                };
                Some(info)
            }
            None => self.inner.read().ivf_config.as_ref().map(|_| IndexInfo {
                index_kind: 0,
                lists: 0,
                nprobe_default: 0,
                built_at_live_count: 0,
                last_drift_ratio: None,
            }),
        }
    }

    /// Search: probe over the closest IVF lists when an index is
    /// published, otherwise the exact full scan. Both paths share
    /// identical post-processing (payload filter, score threshold, top-K,
    /// offset/limit) through [`Self::finish_candidates`], so scores and
    /// semantics are path-independent.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        // Take the metadata and all per-file views inside one read-lock
        // acquisition so a concurrent background compaction (which swaps
        // every file under the write lock) cannot produce a mixed-generation
        // view. Only the atomic loads happen under the lock; the actual scan
        // runs on immutable snapshots.
        let (dim, metric, segment_slots, next_slot, tombstones, vsnap, keysnap, paysnap, ivf) = {
            let inner = self.inner.read();
            if query.vector.len() != inner.meta.vector_size {
                return Err(VectorSearchError::InvalidVectorDimension {
                    expected: inner.meta.vector_size,
                    actual: query.vector.len(),
                });
            }
            (
                inner.meta.vector_size,
                inner.meta.distance,
                inner.meta.segment_slots,
                inner.meta.next_slot,
                self.tombstones.load(),
                self.vectors.snapshot(),
                self.keys.snapshot(),
                self.payloads.snapshot(),
                self.ivf.load(),
            )
        };
        if let Some(index) = ivf.as_ref() {
            let results = self.search_ivf(
                index.as_ref(),
                query,
                dim,
                metric,
                segment_slots,
                next_slot,
                &tombstones,
                &vsnap,
                &keysnap,
                &paysnap,
            );
            drop(ivf);
            return results;
        }
        drop(ivf);

        // 1. Parallel exact scan, skipping tombstones.
        let candidates: Vec<(f32, u32)> = (0..next_slot as u32)
            .into_par_iter()
            .filter(|s| !tombstones.bit(*s as usize))
            .map(|s| {
                let v =
                    Vectors::read_slot(&vsnap, s as u64, segment_slots, dim).ok_or_else(|| {
                        VectorSearchError::CorruptData(format!("slot {s} out of vectors.bin range"))
                    })?;
                let dist = crate::distance::distance(metric, &query.vector, v);
                Ok((crate::distance::to_score(metric, dist), s))
            })
            .collect::<Result<Vec<_>>>()?;

        self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones: &tombstones,
                vsnap: &vsnap,
                keysnap: &keysnap,
                paysnap: &paysnap,
            },
        )
    }

    /// IVF path: probe the `nprobe` closest lists (+ pending slots), then
    /// shared post-processing. With a payload filter that leaves fewer than
    /// `limit` results while unprobed lists remain, nprobe is doubled once as
    /// a bounded accuracy fallback; beyond that the approximate semantics of
    /// IVFFlat apply (`nprobe = lists` degenerates to exact).
    #[allow(clippy::too_many_arguments)]
    fn search_ivf(
        &self,
        index: &IvfIndex,
        query: &SearchQuery,
        dim: usize,
        metric: DistanceMetric,
        segment_slots: u32,
        next_slot: u64,
        tombstones: &TombstoneBits,
        vsnap: &[Arc<memmap2::Mmap>],
        keysnap: &DirView,
        paysnap: &DirView,
    ) -> Result<Vec<SearchResult>> {
        let _ = (metric, next_slot);
        let lists = index.list_count();
        let mut nprobe = index.clamp_nprobe(query.nprobe);
        let pending = self.pending.read().clone();

        let candidates = index.probe_candidates(
            &query.vector,
            nprobe,
            &pending,
            tombstones,
            vsnap,
            segment_slots,
        )?;
        let results = self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones,
                vsnap,
                keysnap,
                paysnap,
            },
        )?;

        let short = query.filter.is_some() && results.len() < query.limit;
        if !short || nprobe >= lists {
            return Ok(results);
        }
        // Single controlled retry with a doubled probe width.
        nprobe = (nprobe * 2).min(lists);
        let candidates = index.probe_candidates(
            &query.vector,
            nprobe,
            &pending,
            tombstones,
            vsnap,
            segment_slots,
        )?;
        self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones,
                vsnap,
                keysnap,
                paysnap,
            },
        )
    }

    /// Shared post-processing for both search paths: payload post-filter,
    /// score threshold, top-K heap selection and result assembly.
    fn finish_candidates(
        &self,
        mut candidates: Vec<(f32, u32)>,
        query: &SearchQuery,
        snap: &SearchSnapshot,
    ) -> Result<Vec<SearchResult>> {
        let SearchSnapshot {
            dim,
            segment_slots,
            tombstones,
            vsnap,
            keysnap,
            paysnap,
        } = snap;
        let _ = tombstones;
        // 2. Post-filter on payload, against the snapshots taken by the
        // caller (one compaction generation).
        if let Some(filter) = &query.filter {
            let mut kept = Vec::with_capacity(candidates.len());
            for (score, slot) in candidates {
                let key = Keys::read_key(keysnap, slot as usize)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("slot {slot} has no key"))
                })?;
                let id = PointId::from(key);
                let payload = Payloads::read_payload(paysnap, slot as usize)?;
                if crate::filter::matches(filter, &id, payload.as_ref())? {
                    kept.push((score, slot));
                }
            }
            candidates = kept;
        }

        // 3. Score threshold (lower bound on the output score).
        if let Some(threshold) = query.score_threshold {
            candidates.retain(|(score, _)| *score >= threshold);
        }

        // 4. Top-K by score (K = offset + limit), then sort descending.
        let k = query.offset.unwrap_or(0).saturating_add(query.limit);
        let top: Vec<(f32, u32)> = if candidates.len() <= k {
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
            candidates
        } else {
            let mut heap: BinaryHeap<std::cmp::Reverse<ScoredSlot>> = BinaryHeap::new();
            for (score, slot) in candidates {
                let s = ScoredSlot { score, slot };
                if heap.len() < k {
                    heap.push(std::cmp::Reverse(s));
                } else if heap.peek().is_some_and(|top| s > top.0) {
                    heap.pop();
                    heap.push(std::cmp::Reverse(s));
                }
            }
            let mut v: Vec<(f32, u32)> = heap
                .into_iter()
                .map(|std::cmp::Reverse(s)| (s.score, s.slot))
                .collect();
            v.sort_by(|a, b| b.0.total_cmp(&a.0));
            v
        };

        // 5. Assemble results against the same snapshots the scan used.
        let with_payload = query
            .with_payload
            .unwrap_or(crate::types::DEFAULT_WITH_PAYLOAD);
        let with_vector = query.with_vector.unwrap_or(false);
        let mut results = Vec::new();
        for (score, slot) in top
            .into_iter()
            .skip(query.offset.unwrap_or(0))
            .take(query.limit)
        {
            let key = Keys::read_key(keysnap, slot as usize)?
                .ok_or_else(|| VectorSearchError::CorruptData(format!("slot {slot} has no key")))?;
            let mut result = SearchResult::new(PointId::from(key), score);
            if with_payload {
                if let Some(payload) = Payloads::read_payload(paysnap, slot as usize)? {
                    result = result.with_payload(payload);
                }
            }
            if with_vector {
                let v = Vectors::read_slot(vsnap, slot as u64, *segment_slots, *dim).ok_or_else(
                    || {
                        VectorSearchError::CorruptData(format!(
                            "slot {slot} out of vectors.bin range"
                        ))
                    },
                )?;
                result = result.with_vector(v.to_vec());
            }
            results.push(result);
        }
        Ok(results)
    }
}

/// A (score, slot) pair ordered by score for the top-K heap.
#[derive(Clone, Copy, Debug)]
struct ScoredSlot {
    score: f32,
    slot: u32,
}

impl PartialEq for ScoredSlot {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.slot == other.slot
    }
}

impl Eq for ScoredSlot {}

impl PartialOrd for ScoredSlot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredSlot {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.slot.cmp(&other.slot))
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
