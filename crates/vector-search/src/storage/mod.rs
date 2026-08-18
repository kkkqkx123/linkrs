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
mod vectors;
mod wal;

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bitvec::prelude::*;
use parking_lot::RwLock;
use rayon::prelude::*;

pub(crate) use meta::Meta;
pub use wal::{Wal, WalPoint, WalRecord, WalTxn};

use crate::error::{Result, VectorSearchError};
use crate::types::{
    CollectionConfig, DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint,
};

use self::compaction::{plan_slots, write_dir_file, write_vectors_file, DirEntry};
use self::directory::{KEY_REC_SIZE, SLOT_REC_SIZE};
use self::keys::Keys;
use self::payloads::Payloads;
use self::vectors::Vectors;

/// Tombstone ratio above which deletes trigger compaction.
const COMPACTION_THRESHOLD: f64 = 0.20;

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
    wal: Wal,
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

        let meta = Meta::new_with_segment_slots(
            collection,
            config.vector_size,
            config.distance,
            segment_slots,
        );
        meta.save(dir)?;

        let vectors = Vectors::create(
            &dir.join("vectors.bin"),
            config.vector_size,
            meta.segment_slots,
        )?;
        let keys = Keys::create(&dir.join("keys.bin"), meta.slot_capacity)?;
        let payloads = Payloads::create(&dir.join("payloads.bin"), meta.slot_capacity)?;

        let tombstones = ArcSwap::from(Arc::new(bitvec::bitvec![0; meta.slot_capacity as usize]));
        let wal = Wal::open_or_create(&dir.join("wal.bin"))?;
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
            wal,
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
            inner: RwLock::new(StoreInner { meta, reverse }),
            tombstones: ArcSwap::from(Arc::new(tombstones)),
            vectors,
            keys,
            payloads,
            wal: Wal::open_or_create(&dir.join("wal.bin"))?,
        };
        store.replay_wal()?;
        Ok(store)
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
        let should_compact = {
            let mut inner = self.inner.write();
            let slot = match inner.reverse.remove(id) {
                Some(slot) => slot,
                None => return Ok(false),
            };
            self.apply_delete_slot_locked(&mut inner, slot)?;
            threshold_met(&inner.meta)
        };
        if should_compact {
            self.compact()?;
        }
        Ok(true)
    }

    /// Apply a WAL-backed transaction (commit protocol §4.6):
    ///
    /// 1. validate every op (no invalid data may be logged);
    /// 2. append the whole `WalTxn` to `wal.bin` + fsync;
    /// 3. apply to memory and the mmap files;
    /// 4. persist `meta.bin` (with `last_applied_txn` advanced).
    ///
    /// On success the caller's buffer may be drained; on failure nothing was
    /// applied in memory and the call may be retried (replay is idempotent).
    pub fn apply_txn(&self, txn: &WalTxn) -> Result<()> {
        let should_compact = {
            let mut inner = self.inner.write();
            for op in &txn.ops {
                if let WalRecord::Upsert { point } = op {
                    let point = point.to_point()?;
                    validate_point(&inner.meta, &point)?;
                }
            }
            self.wal.append(txn)?;
            self.apply_records_locked(&mut inner, &txn.ops)?;
            inner.meta.last_applied_txn = txn.txn_id;
            inner.meta.save(&self.dir)?;
            threshold_met(&inner.meta)
        };
        if should_compact {
            self.compact()?;
        }
        Ok(())
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
        let tomb = self.tombstones.load().count_ones() as u64;
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

    /// Physically remove tombstoned slots and rebuild all files with compacted
    /// slot numbering `0..live_count`.
    ///
    /// Runs under the write lock (blocking searches, acceptable for the
    /// single-node phase A). Procedure (§4.7):
    /// 1. write `vectors_tmp.bin`/`keys_tmp.bin`/`payloads_tmp.bin`;
    /// 2. fsync each and rename over the live file (atomic swap);
    /// 3. rebuild mmap snapshots, `reverse` map and tombstone bitmap;
    /// 4. rewrite `meta.bin`;
    /// 5. append a `Compact` checkpoint to the WAL and truncate it.
    ///
    /// Returns the number of live points after compaction.
    pub fn compact(&self) -> Result<u64> {
        let mut inner = self.inner.write();
        if inner.meta.tombstone_count == 0 || inner.meta.next_slot == 0 {
            return Ok(inner.meta.live_count);
        }
        let dim = inner.meta.vector_size;
        let segment_slots = inner.meta.segment_slots;

        let tombstones = self.tombstones.load();
        let (new_capacity, map) = plan_slots(&tombstones, inner.meta.next_slot, segment_slots);
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
            .store(Arc::new(bitvec![0; new_capacity as usize]));
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

        // 5. WAL checkpoint + truncate
        self.wal.append(&WalTxn {
            txn_id: inner.meta.last_applied_txn,
            ops: vec![WalRecord::Compact],
        })?;
        self.wal.truncate()?;

        Ok(live_count)
    }

    /// Tier 0 exact scan (§6): parallel full scan with the SIMD distance
    /// kernel, post-filter on payload, score threshold, then top-K heap
    /// selection and offset/limit slicing.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let (dim, metric, segment_slots, next_slot) = {
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
            )
        };
        let tombstones = self.tombstones.load();
        let vsnap = self.vectors.snapshot();

        // 1. Parallel scan, skipping tombstones.
        let mut candidates: Vec<(f32, u32)> = (0..next_slot as u32)
            .into_par_iter()
            .filter(|s| !tombstones[*s as usize])
            .map(|s| {
                let v =
                    Vectors::read_slot(&vsnap, s as u64, segment_slots, dim).ok_or_else(|| {
                        VectorSearchError::CorruptData(format!("slot {s} out of vectors.bin range"))
                    })?;
                let dist = crate::distance::distance(metric, &query.vector, v);
                Ok((crate::distance::to_score(metric, dist), s))
            })
            .collect::<Result<Vec<_>>>()?;
        drop(vsnap);
        drop(tombstones);

        // 2. Post-filter on payload.
        if let Some(filter) = &query.filter {
            let psnap = self.payloads.snapshot();
            let keysnap = self.keys.snapshot();
            let mut kept = Vec::with_capacity(candidates.len());
            for (score, slot) in candidates {
                let key = Keys::read_key(&keysnap, slot as usize)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("slot {slot} has no key"))
                })?;
                let id = PointId::from(key);
                let payload = Payloads::read_payload(&psnap, slot as usize)?;
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

        // 5. Assemble results (payload/vector only when requested).
        let with_payload = query.with_payload.unwrap_or(false);
        let with_vector = query.with_vector.unwrap_or(false);
        let psnap = self.payloads.snapshot();
        let keysnap = self.keys.snapshot();
        let vsnap = self.vectors.snapshot();
        let mut results = Vec::new();
        for (score, slot) in top
            .into_iter()
            .skip(query.offset.unwrap_or(0))
            .take(query.limit)
        {
            let key = Keys::read_key(&keysnap, slot as usize)?
                .ok_or_else(|| VectorSearchError::CorruptData(format!("slot {slot} has no key")))?;
            let mut result = SearchResult::new(PointId::from(key), score);
            if with_payload {
                if let Some(payload) = Payloads::read_payload(&psnap, slot as usize)? {
                    result = result.with_payload(payload);
                }
            }
            if with_vector {
                let v = Vectors::read_slot(&vsnap, slot as u64, segment_slots, dim).ok_or_else(
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

fn validate_collection_name(name: &str) -> Result<()> {
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
        // 1 tombstone out of 3 slots exceeds the 20% threshold, so the delete
        // triggers compaction immediately: tombstone_count is back to zero and
        // slot numbering is compacted.
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

        // The single-slot delete crosses the 20% threshold and compacts to an
        // empty collection; the re-insert takes slot 0 of the compacted
        // layout (tombstoned slots are only reused by compaction).
        store.upsert(&point(1, 4)).unwrap();
        assert_eq!(store.count(), 1);
        let meta = store.meta();
        assert_eq!(meta.tombstone_count, 0);
        assert_eq!(meta.next_slot, 1);
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
