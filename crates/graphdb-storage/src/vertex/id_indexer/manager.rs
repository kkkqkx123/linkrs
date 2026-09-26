//! Core concurrency structure of the primary-key index: the striped key
//! maps plus the shared allocation core, and the `IdManager` that drives
//! them.

use std::collections::{BTreeSet, HashMap};

use parking_lot::Mutex;

use graphdb_core::error::{StorageError, StorageResult};

use super::codec;
use super::config::IdIndexerConfig;
use super::key::{validate_key_shape, IdKey};
use super::lookup::PkLookup;

/// Hash segments for the primary-key map. Probes hash to one segment and
/// hold only that segment; inserts hold the key's segment plus the short
/// shared allocation critical section. Lock order is always segments in
/// increasing index order, then the shared core, never the reverse.
pub const ID_STRIPE_COUNT: usize = 16;

pub fn stripe_index(key: &IdKey) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % ID_STRIPE_COUNT
}

/// One committed index mutation since the last baseline flush.
#[derive(Debug, Clone)]
pub(super) enum IndexDelta {
    Insert { key: IdKey, id: u32 },
    Remove { key: IdKey },
}

/// Shared allocation state behind the striped key map: slot table, live
/// set, free stack, delta log and config. Guarded by one short critical
/// section; keyed writes hold their stripe plus this core, probes hold
/// only their stripe.
#[derive(Debug)]
pub(super) struct SharedCore {
    pub(super) keys: Vec<Option<IdKey>>,
    pub(super) live_ids: BTreeSet<u32>,
    pub(super) free_ids: Vec<u32>,
    pub(super) reuse_count: u64,
    pub(super) delta_log: Vec<IndexDelta>,
    pub(super) baseline_invalidated: bool,
    pub(super) config: IdIndexerConfig,
}

impl SharedCore {
    pub(super) fn clone_from_fresh(&mut self, fresh: &SharedCore) {
        self.keys = fresh.keys.clone();
        self.live_ids = fresh.live_ids.clone();
        self.free_ids = fresh.free_ids.clone();
        self.reuse_count = fresh.reuse_count;
        self.delta_log = fresh.delta_log.clone();
        self.baseline_invalidated = fresh.baseline_invalidated;
        self.config = fresh.config.clone();
    }
}

#[derive(Debug)]
pub struct IdManager {
    pub(super) stripes: Vec<Mutex<HashMap<IdKey, u32>>>,
    pub(super) core: Mutex<SharedCore>,
}

impl IdManager {
    pub fn new() -> Self {
        Self::with_config(IdIndexerConfig::default())
    }

    pub fn with_config(config: IdIndexerConfig) -> Self {
        let capacity = config.initial_capacity.min(config.max_capacity);
        let per_stripe = capacity.div_ceil(ID_STRIPE_COUNT).max(1);
        let mut stripes = Vec::with_capacity(ID_STRIPE_COUNT);
        for _ in 0..ID_STRIPE_COUNT {
            stripes.push(Mutex::new(HashMap::with_capacity(per_stripe)));
        }
        Self {
            stripes,
            core: Mutex::new(SharedCore {
                keys: Vec::with_capacity(capacity),
                live_ids: BTreeSet::new(),
                free_ids: Vec::new(),
                reuse_count: 0,
                delta_log: Vec::new(),
                baseline_invalidated: false,
                config,
            }),
        }
    }

    /// Pre-allocate capacity for `additional` more entries in both the Vec and HashMap.
    /// This avoids repeated rehashing during batch inserts.
    pub fn reserve(&self, additional: usize) {
        {
            let mut core = self.core.lock();
            let target = core.keys.len().saturating_add(additional);
            if target > core.keys.capacity() {
                let new_cap = target.min(core.config.max_capacity);
                let grow = new_cap.saturating_sub(core.keys.capacity());
                if grow > 0 {
                    core.keys.reserve(grow);
                }
            }
        }
        let per_stripe = additional.div_ceil(ID_STRIPE_COUNT).max(1);
        for stripe in &self.stripes {
            stripe.lock().reserve(per_stripe);
        }
    }

    pub fn insert(&self, key: IdKey) -> StorageResult<u32> {
        validate_key_shape(&key)?;
        let stripe = stripe_index(&key);
        let mut map = self.stripes[stripe].lock();
        if map.contains_key(&key) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        let mut core = self.core.lock();
        let id = Self::take_next_id_locked(&mut core)?;
        Self::bind_slot_locked(&mut core, &mut map, key, id);
        Ok(id)
    }

    /// Acquire an unbound local id, preferring a deleted slot from the free
    /// stack and growing the high-water mark otherwise. The slot stays
    /// unbound (`keys` reports `None`) for the caller to bind later or
    /// return through [`Self::release_reserved`].
    ///
    /// Reuse is ordered: the largest free id (closest to the high-water
    /// mark) is claimed first, so recycled slots refill the allocation
    /// tail instead of scattering holes across the id space. Sequential
    /// scans stay dense under delete-plus-reinsert churn; the free stack
    /// itself keeps insertion order and only the claim is ordered.
    fn take_next_id_locked(core: &mut SharedCore) -> StorageResult<u32> {
        // Lazy ID reuse: recycle a deleted slot before growing the id space.
        if !core.free_ids.is_empty() {
            let mut best = 0usize;
            for (i, id) in core.free_ids.iter().enumerate() {
                if *id > core.free_ids[best] {
                    best = i;
                }
            }
            let recycled = core.free_ids.swap_remove(best);
            core.reuse_count = core.reuse_count.saturating_add(1);
            let idx = recycled as usize;
            if idx >= core.keys.len() {
                // Recycled idx at the tail (rare after deserialize
                // rebuilding): extend so the slot exists unbound.
                while core.keys.len() <= idx {
                    core.keys.push(None);
                }
            } else {
                debug_assert!(core.keys[idx].is_none());
            }
            return Ok(recycled);
        }

        if core.keys.len() >= core.config.max_capacity {
            return Err(StorageError::capacity_exceeded());
        }

        if core.keys.len() >= core.keys.capacity() {
            let current_capacity = core.keys.capacity();
            if current_capacity >= core.config.max_capacity {
                return Err(StorageError::capacity_exceeded());
            }

            let new_capacity = ((current_capacity as f64 * core.config.growth_factor) as usize)
                .min(core.config.max_capacity)
                .max(current_capacity + 1);
            core.keys.reserve(new_capacity - current_capacity);
        }

        let index = core.keys.len() as u32;
        core.keys.push(None);
        Ok(index)
    }

    /// Bind a key to a slot obtained from [`Self::take_next_id_locked`],
    /// recording the committed-insert delta entry. Caller holds the key's
    /// stripe plus the core, in stripe-before-core order.
    fn bind_slot_locked(core: &mut SharedCore, map: &mut HashMap<IdKey, u32>, key: IdKey, id: u32) {
        // Reuse audit: a recycled slot must be fully detached (no key, not
        // live) before a new key claims it; otherwise a dangling reference
        // to the previous occupant survives the reuse.
        debug_assert!(
            matches!(core.keys.get(id as usize), Some(None)),
            "bind_slot claimed an occupied slot {id}"
        );
        debug_assert!(
            !core.live_ids.contains(&id),
            "bind_slot claimed a live slot {id}"
        );
        core.keys[id as usize] = Some(key.clone());
        map.insert(key.clone(), id);
        core.live_ids.insert(id);
        core.delta_log.push(IndexDelta::Insert { key, id });
    }

    /// Acquire an unbound local id (see [`Self::take_next_id_locked`]).
    pub fn reserve_next(&self) -> StorageResult<u32> {
        let mut core = self.core.lock();
        Self::take_next_id_locked(&mut core)
    }

    /// Bind an external key to a previously reserved id at commit apply.
    /// Fails when the key is already bound or the id is not a currently
    /// unbound slot, so a stale or foreign reservation cannot clobber live
    /// mappings. Stripe before core.
    pub fn register_reserved(&self, key: IdKey, id: u32) -> StorageResult<()> {
        validate_key_shape(&key)?;
        let stripe = stripe_index(&key);
        let mut map = self.stripes[stripe].lock();
        if map.contains_key(&key) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        let mut core = self.core.lock();
        if !matches!(core.keys.get(id as usize), Some(None)) {
            return Err(StorageError::invalid_operation(format!(
                "reserved id {id} does not name an unbound slot"
            )));
        }
        Self::bind_slot_locked(&mut core, &mut map, key, id);
        Ok(())
    }

    /// Return an unbound reserved id to the free stack. Bound ids are a
    /// no-op, making release safe to call on slots whose reservation was
    /// already consumed by a bind; releasing a double reservation would
    /// only add a duplicate free-stack entry the next pop turns into a
    /// no-op bind conflict, so callers keep single ownership.
    /// Core-only: no stripe is held.
    pub fn release_reserved(&self, id: u32) {
        let mut core = self.core.lock();
        if matches!(core.keys.get(id as usize), Some(None)) {
            core.free_ids.push(id);
        }
    }

    /// Cancel a previous release: pull the id back out of the free stack
    /// while its slot is still unbound. Only frees-and-reclaims a still
    /// pending release; any other unbound state (another row's live
    /// reservation or a never-released hole) reports false and the caller
    /// must reserve a fresh id instead. Core-only.
    pub fn try_reclaim(&self, id: u32) -> bool {
        let mut core = self.core.lock();
        if !matches!(core.keys.get(id as usize), Some(None)) {
            return false;
        }
        match core.free_ids.iter().rposition(|free| *free == id) {
            Some(pos) => {
                core.free_ids.swap_remove(pos);
                true
            }
            None => false,
        }
    }

    /// Visibility-aware lookup: the global committed area gated by the
    /// caller's row-visibility predicate. Collapses the old two-step read
    /// into one two-state call. Stripe-only: probes on different segments
    /// proceed concurrently without touching the shared core.
    pub fn lookup(&self, key: &IdKey, is_visible: impl Fn(u32) -> bool) -> PkLookup {
        let stripe = stripe_index(key);
        match self.stripes[stripe].lock().get(key).copied() {
            Some(id) if is_visible(id) => PkLookup::Visible(id),
            _ => PkLookup::Missing,
        }
    }

    /// Stripe-only probe without visibility filtering.
    pub fn get_id(&self, key: &IdKey) -> Option<u32> {
        let stripe = stripe_index(key);
        self.stripes[stripe].lock().get(key).copied()
    }

    /// Core-only reverse lookup.
    pub fn get_key(&self, index: u32) -> Option<IdKey> {
        self.core.lock().keys.get(index as usize)?.as_ref().cloned()
    }

    pub fn len(&self) -> usize {
        self.core.lock().live_ids.len()
    }

    /// High-water mark of the local id space (holes from deletions
    /// included). Deleted slots are recycled through the free stack on
    /// insert, so live row IDs stay stable until a watermark-gated
    /// compaction re-densifies them. Core-only.
    pub fn next_index(&self) -> u32 {
        self.core.lock().keys.len() as u32
    }

    /// Stripe before core: unbind the key from its segment, then clear the
    /// slot and recycle the id under the shared core.
    pub fn remove(&self, key: &IdKey) -> Option<u32> {
        let stripe = stripe_index(key);
        let mut map = self.stripes[stripe].lock();
        let idx = map.remove(key)?;
        let mut core = self.core.lock();
        if (idx as usize) < core.keys.len() {
            core.keys[idx as usize] = None;
        }
        core.live_ids.remove(&idx);
        core.free_ids.push(idx);
        core.delta_log.push(IndexDelta::Remove { key: key.clone() });
        Some(idx)
    }

    /// Stripes in index order; core is not needed for the live mappings.
    pub fn iter(&self) -> Vec<(IdKey, u32)> {
        let mut out = Vec::new();
        for stripe in &self.stripes {
            out.extend(stripe.lock().iter().map(|(key, &idx)| (key.clone(), idx)));
        }
        out
    }

    /// Index-level live IDs in ascending order (see `IdIndexer::live_ids`).
    /// Core-only snapshot of the live set.
    pub fn live_ids(&self) -> Vec<u32> {
        self.core.lock().live_ids.iter().copied().collect()
    }

    /// Pure dense-mapping computation without mutating the index.
    ///
    /// Sorting live entries ascending and assigning dense `0..n` yields the
    /// old-to-new mapping for rows that move; unmoved rows are absent.
    /// The compaction coordinator calls this first so replacements for the
    /// timestamp and column structures can be built before any mutation,
    /// keeping the three structures atomically consistent. Core-only.
    pub fn compute_compact_mapping(&self) -> HashMap<u32, u32> {
        let core = self.core.lock();
        let mut olds: Vec<u32> = core.live_ids.iter().copied().collect();
        if olds.is_empty() {
            return HashMap::new();
        }
        olds.sort_unstable();
        let mut mapping = HashMap::new();
        for (new_id, old_id) in olds.into_iter().enumerate() {
            let new_id_u32 = new_id as u32;
            if old_id != new_id_u32 {
                mapping.insert(old_id, new_id_u32);
            }
        }
        mapping
    }

    pub fn set_at(&self, index: u32, key: IdKey) {
        let stripe = stripe_index(&key);
        let mut map = self.stripes[stripe].lock();
        if map.contains_key(&key) {
            return;
        }
        let mut core = self.core.lock();
        while core.keys.len() <= index as usize {
            core.keys.push(None);
        }
        core.keys[index as usize] = Some(key.clone());
        map.insert(key, index);
        core.live_ids.insert(index);
    }

    /// Committed mutations since the last baseline flush. Core-only.
    pub fn delta_len(&self) -> usize {
        self.core.lock().delta_log.len()
    }

    /// Scale-aware anchor threshold: the fixed floor bounds replay cost on
    /// small tables, while the proportional term keeps the delta a bounded
    /// fraction of the index on large tables where a fixed count would
    /// either anchor far too often (write amplification) or far too rarely
    /// (replay cost and double-stored key memory).
    pub fn anchor_threshold_for_live(live: usize) -> usize {
        super::PK_DELTA_ANCHOR_THRESHOLD.max(live / 4)
    }

    /// Anchor check against an explicit live size, for flush paths that
    /// already hold the count. Consumes the invalidation flag. Core-only.
    pub fn should_anchor_baseline_for_live(&self, live: usize) -> bool {
        let mut core = self.core.lock();
        let invalidated = std::mem::take(&mut core.baseline_invalidated);
        invalidated || core.delta_log.len() >= Self::anchor_threshold_for_live(live)
    }

    /// Cumulative free-stack reuses since creation (see `reuse_count`).
    /// Core-only.
    pub fn reuse_count(&self) -> u64 {
        self.core.lock().reuse_count
    }

    /// Non-destructive peek at the compaction-moved-rows flag (see
    /// `baseline_invalidated`). Observability only: the flush trigger
    /// policy reads it without consuming, unlike
    /// [`Self::take_baseline_invalidated`]. Core-only.
    pub fn baseline_invalidated(&self) -> bool {
        self.core.lock().baseline_invalidated
    }

    /// Current free-stack depth: slots awaiting reuse. Core-only.
    pub fn free_depth(&self) -> usize {
        self.core.lock().free_ids.len()
    }

    /// Index-level hole ratio `1 - bound / allocated` over the raw id
    /// space, ignoring timestamp visibility. Fast pre-check for the
    /// hole-rate watermark: when this is below the watermark, the
    /// snapshot-aware ratio cannot be above it, so maintenance can skip
    /// the timestamp scan. Ordered reuse keeps this low by refilling
    /// tail-adjacent holes first. Core-only.
    pub fn hole_ratio(&self) -> f64 {
        let core = self.core.lock();
        let allocated = core.keys.len();
        if allocated == 0 {
            return 0.0;
        }
        let bound = core.live_ids.len();
        if bound >= allocated {
            return 0.0;
        }
        1.0 - (bound as f64 / allocated as f64)
    }

    /// Drop the since-baseline delta without persisting it. Baseline-flush
    /// path only: the fresh full snapshot supersedes every delta entry.
    /// Core-only.
    pub fn clear_index_delta(&self) {
        let mut core = self.core.lock();
        core.delta_log.clear();
        core.baseline_invalidated = false;
    }

    pub fn memory_usage(&self) -> usize {
        super::memory::memory_breakdown(self).total_bytes
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }

    /// Serialize the since-baseline delta for `id_indexer.delta`.
    /// Core-only snapshot of the delta log.
    #[cfg(test)]
    pub fn serialize_delta(&self) -> Vec<u8> {
        let core = self.core.lock();
        codec::serialize_delta(&core)
    }

    /// Decode delta entries. Corrupt bytes fail the whole delta so the
    /// caller refuses the open.
    pub fn deserialize_delta(data: &[u8]) -> StorageResult<Vec<(u8, u32, IdKey)>> {
        codec::deserialize_delta(data)
    }

    /// Apply decoded delta entries onto the loaded baseline without
    /// recording them again (replay must not extend the live delta log).
    /// Insert restores the exact baseline id; remove drops the key and
    /// recycles its id. All stripes in index order, then the core.
    pub fn apply_delta_entries(&self, entries: &[(u8, u32, IdKey)]) -> StorageResult<()> {
        let mut guards: Vec<_> = self.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.core.lock();
        for (op, id, key) in entries {
            match op {
                0 => {
                    validate_key_shape(key)?;
                    let stripe = stripe_index(key);
                    if let Some(existing) = guards[stripe].get(key).copied() {
                        if existing != *id {
                            return Err(StorageError::deserialize_error(format!(
                                "pk delta insert diverges for {:?}: baseline {} vs delta {}",
                                key, existing, id
                            )));
                        }
                        continue;
                    }
                    while core.keys.len() <= *id as usize {
                        core.keys.push(None);
                    }
                    core.keys[*id as usize] = Some(key.clone());
                    guards[stripe].insert(key.clone(), *id);
                    core.live_ids.insert(*id);
                }
                1 => {
                    let stripe = stripe_index(key);
                    if let Some(idx) = guards[stripe].remove(key) {
                        if (idx as usize) < core.keys.len() {
                            core.keys[idx as usize] = None;
                        }
                        core.live_ids.remove(&idx);
                        core.free_ids.push(idx);
                    }
                }
                _ => {
                    return Err(StorageError::deserialize_error(format!(
                        "pk delta unknown op {}",
                        op
                    )));
                }
            }
        }
        Ok(())
    }

    /// Deserialize from bytes, rebuilding the index.
    pub fn deserialize(data: &[u8]) -> StorageResult<Self> {
        codec::deserialize(data)
    }
}

impl Default for IdManager {
    fn default() -> Self {
        Self::new()
    }
}
