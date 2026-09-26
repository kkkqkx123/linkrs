//! ID Indexer
//!
//! Maps external IDs (strings or integers) to internal vertex IDs.
//! Provides O(1) lookup in both directions.
//!
//! # Architecture
//!
//! This module uses a two-level design:
//!
//! - **IdManager**: Core business logic for ID management
//!   - Bidirectional mapping between external IDs and internal indices
//!   - Compact/remapping algorithm
//!   - Uses HashMap for storage (no unnecessary concurrency)
//!
//! - **IdIndexer**: Shared handle for direct index users
//!   - Internal lock allows sharing across threads for index-only work
//!   - Table-level consistency still needs the outer shard lock

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;

use graphdb_core::error::{StorageError, StorageResult};
use graphdb_core::types::VERTEX_ID_MAX_SIZE;

const DEFAULT_INITIAL_CAPACITY: usize = 1024;
const DEFAULT_GROWTH_FACTOR: f64 = 1.5;
const MAX_CAPACITY: usize = u32::MAX as usize;

/// Since-baseline delta entries beyond this count force the next
/// incremental flush to anchor a new full baseline instead of extending
/// the delta file. Bounds delta replay cost, delta file size, and the
/// double-stored key memory between baselines.
pub const PK_DELTA_ANCHOR_THRESHOLD: usize = 8192;

const ID_KEY_TYPE_INT: u8 = 0;
const ID_KEY_TYPE_TEXT: u8 = 1;

/// Visibility-aware primary-key lookup result.
///
/// Collapses the old two-step read (`get_index` plus a timestamp check in
/// the caller) into one call: committed bindings passing the caller's
/// visibility predicate report as [`PkLookup::Visible`], everything else is
/// [`PkLookup::Missing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkLookup {
    /// Committed binding visible at the read timestamp.
    Visible(u32),
    /// No binding, or a committed binding invisible at the read timestamp.
    Missing,
}

impl PkLookup {
    /// Visible id, if any.
    pub fn visible_id(self) -> Option<u32> {
        match self {
            Self::Visible(id) => Some(id),
            Self::Missing => None,
        }
    }
}

/// One committed index mutation since the last baseline flush.
#[derive(Debug, Clone)]
enum IndexDelta {
    Insert { key: IdKey, id: u32 },
    Remove { key: IdKey },
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum IdKey {
    Int(i64),
    Text(String),
}

impl IdKey {
    /// Write the key bytes into an existing buffer to avoid extra allocations.
    /// The buffer is cleared before writing.
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        buf.clear();
        match self {
            IdKey::Int(val) => {
                buf.reserve(9);
                buf.push(ID_KEY_TYPE_INT);
                buf.extend_from_slice(&val.to_be_bytes());
            }
            IdKey::Text(val) => {
                buf.reserve(1 + val.len());
                buf.push(ID_KEY_TYPE_TEXT);
                buf.extend_from_slice(val.as_bytes());
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.is_empty() {
            return Err(StorageError::deserialize_error(
                "Empty IdKey bytes".to_string(),
            ));
        }

        match bytes[0] {
            ID_KEY_TYPE_INT => {
                if bytes.len() != 9 {
                    return Err(StorageError::deserialize_error(format!(
                        "Invalid Int IdKey length: {}",
                        bytes.len()
                    )));
                }
                let val_bytes: [u8; 8] = bytes[1..9].try_into().map_err(|_| {
                    StorageError::deserialize_error("Invalid Int IdKey bytes".to_string())
                })?;
                Ok(IdKey::Int(i64::from_be_bytes(val_bytes)))
            }
            ID_KEY_TYPE_TEXT => {
                let text = String::from_utf8(bytes[1..].to_vec())
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
                Ok(IdKey::Text(text))
            }
            tag => Err(StorageError::deserialize_error(format!(
                "Unknown IdKey type tag: {}",
                tag
            ))),
        }
    }
}

impl std::fmt::Display for IdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdKey::Int(val) => write!(f, "{}", val),
            IdKey::Text(val) => write!(f, "{}", val),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdIndexerConfig {
    pub initial_capacity: usize,
    pub growth_factor: f64,
    pub max_capacity: usize,
}

impl Default for IdIndexerConfig {
    fn default() -> Self {
        Self {
            initial_capacity: DEFAULT_INITIAL_CAPACITY,
            growth_factor: DEFAULT_GROWTH_FACTOR,
            max_capacity: MAX_CAPACITY,
        }
    }
}

impl IdIndexerConfig {
    pub fn with_initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = capacity;
        self
    }
}

/// Primary-key shape shared by every index mutation path.
///
/// The table layer validates the same shape, but the indexer enforces it
/// again so direct index users and persisted-file replays cannot smuggle in
/// over-long text keys or negative integer keys.
fn validate_key_shape(key: &IdKey) -> StorageResult<()> {
    match key {
        IdKey::Int(id) if *id < 0 => Err(StorageError::invalid_input(format!(
            "Vertex id cannot be negative: {}",
            id
        ))),
        IdKey::Text(id) if id.len() > VERTEX_ID_MAX_SIZE => {
            Err(StorageError::invalid_input(format!(
                "Vertex id exceeds max length of {} bytes: got {} bytes",
                VERTEX_ID_MAX_SIZE,
                id.len()
            )))
        }
        _ => Ok(()),
    }
}

/// Core bidirectional mapping between external IDs and internal indices.
///
/// This struct manages the fundamental lookup operations:
/// - Key → ID mapping (via HashMap)
/// - ID → Key reverse mapping (via Vec)
///
/// IdManager is the authoritative source for ID management logic.
/// Per-component memory accounting for one primary-key index shard.
/// Sums to the value reported by [`IdManager::memory_usage`]; exposed so
/// operators can see which structure (key heap, map, live set, delta log)
/// dominates before the shard crosses its memory budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdIndexMemoryBreakdown {
    pub slot_count: usize,
    pub live_count: usize,
    pub free_depth: usize,
    pub delta_entries: usize,
    pub delta_heap_bytes: usize,
    pub keys_heap_bytes: usize,
    pub map_bytes: usize,
    pub set_bytes: usize,
    pub free_bytes: usize,
    pub total_bytes: usize,
}

/// Hash segments for the primary-key map. Probes hash to one segment and
/// hold only that segment; inserts hold the key's segment plus the short
/// shared allocation critical section. Lock order is always segments in
/// increasing index order, then the shared core, never the reverse.
pub const ID_STRIPE_COUNT: usize = 16;

fn stripe_index(key: &IdKey) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % ID_STRIPE_COUNT
}

/// Shared allocation state behind the striped key map: slot table, live
/// set, free stack, delta log and config. Guarded by one short critical
/// section; keyed writes hold their stripe plus this core, probes hold
/// only their stripe.
#[derive(Debug)]
struct SharedCore {
    keys: Vec<Option<IdKey>>,
    live_ids: BTreeSet<u32>,
    free_ids: Vec<u32>,
    reuse_count: u64,
    delta_log: Vec<IndexDelta>,
    baseline_invalidated: bool,
    config: IdIndexerConfig,
}

#[derive(Debug)]
pub struct IdManager {
    stripes: Vec<Mutex<HashMap<IdKey, u32>>>,
    core: Mutex<SharedCore>,
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
    fn bind_slot_locked(
        core: &mut SharedCore,
        map: &mut HashMap<IdKey, u32>,
        key: IdKey,
        id: u32,
    ) {
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

    /// Reserve a local id for a not-yet-committed row without binding any
    /// external key: nothing is visible to lookups, `live_ids`, or the delta
    /// log until [`Self::register_reserved`] binds the key at commit apply.
    /// Core-only: no stripe is held.
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

    /// All stripes in index order, then the core. Exclusive path only
    /// (compaction runs barriered); point probes block while it holds the
    /// segments.
    pub fn compact(&self) -> StorageResult<HashMap<u32, u32>> {
        let mapping = self.compute_compact_mapping();
        if mapping.is_empty() {
            // Already dense; still clear free list if it had stale entries.
            // Ids did not move, so the since-baseline delta stays valid.
            self.core.lock().free_ids.clear();
            return Ok(HashMap::new());
        }

        let mut guards: Vec<_> = self.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.core.lock();
        let mut entries: Vec<(u32, IdKey)> = Vec::new();
        for map in guards.iter() {
            entries.extend(map.iter().map(|(key, &idx)| (idx, key.clone())));
        }
        entries.sort_by_key(|(old_id, _)| *old_id);
        Self::rebuild_with_mapping_locked(&mut core, &mut guards, &entries)?;
        core.free_ids.clear();
        // Live rows moved: delta entries addressed by old ids are stale.
        // Drop them and force the next flush to anchor a new baseline.
        core.delta_log.clear();
        core.baseline_invalidated = true;

        Ok(mapping)
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

    fn rebuild_with_mapping_locked(
        core: &mut SharedCore,
        guards: &mut [parking_lot::MutexGuard<'_, HashMap<IdKey, u32>>],
        entries: &[(u32, IdKey)],
    ) -> StorageResult<()> {
        let mut new_keys = vec![None; entries.len()];
        for (new_id, (_, key)) in entries.iter().enumerate() {
            new_keys[new_id] = Some(key.clone());
        }
        for map in guards.iter_mut() {
            map.clear();
        }
        for (new_id, (_, key)) in entries.iter().enumerate() {
            let stripe = stripe_index(key);
            guards[stripe].insert(key.clone(), new_id as u32);
        }
        core.keys = new_keys;
        core.live_ids = (0..entries.len() as u32).collect();
        core.free_ids.clear();
        Ok(())
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

    /// Whether compaction moved live rows since the last baseline flush.
    /// Consumes the flag: the next incremental flush anchors a new full
    /// baseline when this returns true. Core-only.
    pub fn take_baseline_invalidated(&self) -> bool {
        std::mem::take(&mut self.core.lock().baseline_invalidated)
    }

    /// Scale-aware anchor threshold: the fixed floor bounds replay cost on
    /// small tables, while the proportional term keeps the delta a bounded
    /// fraction of the index on large tables where a fixed count would
    /// either anchor far too often (write amplification) or far too rarely
    /// (replay cost and double-stored key memory).
    pub fn anchor_threshold_for_live(live: usize) -> usize {
        PK_DELTA_ANCHOR_THRESHOLD.max(live / 4)
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

    /// Per-component memory accounting backing [`Self::memory_usage`].
    /// Stripes in index order, then the core.
    pub fn memory_breakdown(&self) -> IdIndexMemoryBreakdown {
        let mut guards = Vec::with_capacity(ID_STRIPE_COUNT);
        for stripe in &self.stripes {
            guards.push(stripe.lock());
        }
        let core = self.core.lock();
        let mut keys_heap_bytes = 0usize;
        for key_opt in &core.keys {
            if let Some(IdKey::Text(text)) = key_opt {
                keys_heap_bytes += text.len();
            }
        }
        let mut delta_heap_bytes = 0usize;
        for delta in &core.delta_log {
            match delta {
                IndexDelta::Insert { key, .. } | IndexDelta::Remove { key } => {
                    if let IdKey::Text(text) = key {
                        delta_heap_bytes += text.len();
                    }
                }
            }
        }
        let live: usize = guards.iter().map(|m| m.len()).sum();
        let slot_bytes = core.keys.capacity() * std::mem::size_of::<Option<IdKey>>();
        let map_bytes = live * (std::mem::size_of::<IdKey>() + std::mem::size_of::<u32>());
        let set_bytes = core.live_ids.len() * (std::mem::size_of::<u32>() + 32);
        let free_bytes = core.free_ids.capacity() * std::mem::size_of::<u32>();
        IdIndexMemoryBreakdown {
            slot_count: core.keys.len(),
            live_count: live,
            free_depth: core.free_ids.len(),
            delta_entries: core.delta_log.len(),
            delta_heap_bytes,
            keys_heap_bytes,
            map_bytes,
            set_bytes,
            free_bytes,
            total_bytes: slot_bytes
                + keys_heap_bytes
                + delta_heap_bytes
                + map_bytes
                + set_bytes
                + free_bytes,
        }
    }

    /// Drop the since-baseline delta without persisting it. Baseline-flush
    /// path only: the fresh full snapshot supersedes every delta entry.
    /// Core-only.
    pub fn clear_index_delta(&self) {
        let mut core = self.core.lock();
        core.delta_log.clear();
        core.baseline_invalidated = false;
    }

    /// Serialize the since-baseline delta for `id_indexer.delta`.
    ///
    /// Entry encoding reuses the key bytes ([`IdKey::write_to`]): `count:u32`
    /// followed by per-entry `op:u8` (`0` insert with `id:u32`, `1` remove)
    /// plus `key_len:u32` and key bytes. No new key format is introduced.
    /// Core-only snapshot of the delta log.
    pub fn serialize_delta(&self) -> Vec<u8> {
        let core = self.core.lock();
        let mut buf = Vec::new();
        buf.extend_from_slice(&(core.delta_log.len() as u32).to_le_bytes());
        let mut key_buf = Vec::new();
        for delta in &core.delta_log {
            match delta {
                IndexDelta::Insert { key, id } => {
                    buf.push(0u8);
                    buf.extend_from_slice(&id.to_le_bytes());
                    key.write_to(&mut key_buf);
                    buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
                    buf.extend_from_slice(&key_buf);
                }
                IndexDelta::Remove { key } => {
                    buf.push(1u8);
                    key.write_to(&mut key_buf);
                    buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
                    buf.extend_from_slice(&key_buf);
                }
            }
        }
        buf
    }

    /// Decode delta entries. Corrupt bytes fail the whole delta so the
    /// caller refuses the open.
    pub fn deserialize_delta(data: &[u8]) -> StorageResult<Vec<(u8, u32, IdKey)>> {
        let mut cursor = data;
        let take = |cursor: &mut &[u8], len: usize, field: &str| -> StorageResult<Vec<u8>> {
            if len > cursor.len() {
                return Err(StorageError::deserialize_error(format!(
                    "pk delta {} length {} exceeds remaining {}",
                    field,
                    len,
                    cursor.len()
                )));
            }
            let (head, tail) = cursor.split_at(len);
            *cursor = tail;
            Ok(head.to_vec())
        };
        if cursor.len() < 4 {
            return Err(StorageError::deserialize_error(
                "pk delta truncated count".to_string(),
            ));
        }
        let count = u32::from_le_bytes(
            take(&mut cursor, 4, "count")?[..4]
                .try_into()
                .map_err(|_| {
                    StorageError::deserialize_error("pk delta count malformed".to_string())
                })?,
        ) as usize;
        // Sanity bound mirroring the baseline check: every entry needs at
        // least its op, key length, and key tag byte on the wire, so a count
        // the remaining bytes cannot hold is corruption rather than data.
        const MIN_DELTA_ENTRY_BYTES: usize = 6;
        if count > cursor.len() / MIN_DELTA_ENTRY_BYTES {
            return Err(StorageError::deserialize_error(format!(
                "pk delta count {} exceeds wire capacity of {} bytes",
                count,
                cursor.len(),
            )));
        }
        let mut out = Vec::with_capacity(count.min(1 << 20));
        for _ in 0..count {
            let op = take(&mut cursor, 1, "op")?[0];
            if op != 0 && op != 1 {
                return Err(StorageError::deserialize_error(format!(
                    "pk delta unknown op {}",
                    op
                )));
            }
            let id = if op == 0 {
                u32::from_le_bytes(take(&mut cursor, 4, "id")?[..4].try_into().map_err(|_| {
                    StorageError::deserialize_error("pk delta id malformed".to_string())
                })?)
            } else {
                0
            };
            let key_len =
                u32::from_le_bytes(take(&mut cursor, 4, "key_len")?[..4].try_into().map_err(
                    |_| StorageError::deserialize_error("pk delta key_len malformed".to_string()),
                )?) as usize;
            let key_bytes = take(&mut cursor, key_len, "key")?;
            out.push((op, id, IdKey::from_bytes(&key_bytes)?));
        }
        if !cursor.is_empty() {
            return Err(StorageError::deserialize_error(format!(
                "pk delta has {} trailing bytes",
                cursor.len()
            )));
        }
        Ok(out)
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

    pub fn memory_usage(&self) -> usize {
        self.memory_breakdown().total_bytes
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }

    /// Serialize the index to bytes for persistence.
    ///
    /// Format:
    /// - count: u32 (number of entries)
    /// - for each entry:
    ///   - internal_id: u32
    ///   - key_len: u32
    ///   - key_bytes: [u8; key_len]
    pub fn serialize(&self) -> Vec<u8> {
        let core = self.core.lock();
        let mut buf = Vec::new();
        let count = core.live_ids.len() as u32;
        buf.extend_from_slice(&count.to_le_bytes());

        let mut key_buf = Vec::new();
        for (idx, key_opt) in core.keys.iter().enumerate() {
            if let Some(key) = key_opt {
                buf.extend_from_slice(&(idx as u32).to_le_bytes());
                key.write_to(&mut key_buf);
                buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
                buf.extend_from_slice(&key_buf);
            }
        }

        buf
    }

    /// Deserialize from bytes, rebuilding the index.
    pub fn deserialize(data: &[u8]) -> StorageResult<Self> {
        use std::io::Read;

        let mut cursor = data;
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        // Sanity bound: every entry needs at least its id, key length, and
        // key tag byte on the wire, so a count the remaining bytes cannot
        // hold is corruption rather than data. This also bounds the
        // pre-allocation below by the file size.
        const MIN_ENTRY_BYTES: usize = 9;
        if count > data.len().saturating_sub(4) / MIN_ENTRY_BYTES {
            return Err(StorageError::deserialize_error(format!(
                "pk baseline count {} exceeds wire capacity of {} bytes",
                count,
                data.len(),
            )));
        }

        let manager = Self::with_config(IdIndexerConfig::default());
        manager.reserve(count);

        for _ in 0..count {
            let mut id_bytes = [0u8; 4];
            cursor.read_exact(&mut id_bytes)?;
            let internal_id = u32::from_le_bytes(id_bytes);

            let mut key_len_bytes = [0u8; 4];
            cursor.read_exact(&mut key_len_bytes)?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;

            let key = IdKey::from_bytes(&key_bytes)?;
            validate_key_shape(&key)?;
            manager.set_at(internal_id, key);
        }

        // Baselines hold each live key exactly once; fewer bindings than
        // declared means duplicated or colliding keys in a corrupt file.
        if manager.len() != count {
            return Err(StorageError::deserialize_error(format!(
                "pk baseline holds {} bindings for declared count {}",
                manager.len(),
                count
            )));
        }

        // Rebuild free list for holes left by non-dense persisted ids
        // (e.g., after deletions that left gaps).
        {
            let mut core = manager.core.lock();
            core.free_ids = core
                .keys
                .iter()
                .enumerate()
                .filter_map(|(idx, k)| if k.is_none() { Some(idx as u32) } else { None })
                .collect();
        }

        Ok(manager)
    }
}

impl Default for IdManager {
    fn default() -> Self {
        Self::new()
    }
}

/// ID indexer wrapper with striped key segments.
///
/// Cloned handles share the same striped maps plus the shared allocation
/// core. Probes hash to one segment and hold only that segment; keyed
/// writes hold the key's segment plus the short shared core in
/// stripe-before-core order; global operations take all segments in index
/// order then the core. The incremental log still appends in commit order
/// under the core.
#[derive(Debug, Clone)]
pub struct IdIndexer {
    manager: Arc<IdManager>,
}

impl IdIndexer {
    pub fn new() -> Self {
        Self::with_config(IdIndexerConfig::default())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_config(IdIndexerConfig::default().with_initial_capacity(capacity))
    }

    pub fn with_config(config: IdIndexerConfig) -> Self {
        Self {
            manager: Arc::new(IdManager::with_config(config.clone())),
        }
    }

    pub fn insert(&self, key: IdKey) -> StorageResult<u32> {
        self.manager.insert(key)
    }

    /// Reserve a local id without binding a key (see
    /// [`IdManager::reserve_next`]).
    pub fn reserve_next(&self) -> StorageResult<u32> {
        self.manager.reserve_next()
    }

    /// Bind a key to a reserved id at commit apply (see
    /// [`IdManager::register_reserved`]).
    pub fn register_reserved(&self, key: IdKey, id: u32) -> StorageResult<()> {
        self.manager.register_reserved(key, id)
    }

    /// Return an unbound reserved id to the free stack (see
    /// [`IdManager::release_reserved`]).
    pub fn release_reserved(&self, id: u32) {
        self.manager.release_reserved(id);
    }

    /// Claim back a released reservation whose slot is still unbound (see
    /// [`IdManager::try_reclaim`]).
    pub fn try_reclaim(&self, id: u32) -> bool {
        self.manager.try_reclaim(id)
    }

    /// Pre-allocate capacity for `additional` more entries.
    /// Call before batch inserts to avoid repeated rehashing.
    pub fn reserve(&self, additional: usize) {
        self.manager.reserve(additional);
    }

    pub fn get_index(&self, key: &IdKey) -> Option<u32> {
        self.manager.get_id(key)
    }

    pub fn get_key(&self, index: u32) -> Option<IdKey> {
        self.manager.get_key(index)
    }

    pub fn len(&self) -> usize {
        self.manager.len()
    }

    /// Next free local id (see [`IdManager::next_index`]).
    pub fn next_index(&self) -> u32 {
        self.manager.next_index()
    }

    pub fn remove(&self, key: &IdKey) -> Option<u32> {
        self.manager.remove(key)
    }

    /// Visibility-aware two-state lookup.
    pub fn lookup(&self, key: &IdKey, is_visible: impl Fn(u32) -> bool) -> PkLookup {
        self.manager.lookup(key, is_visible)
    }

    /// Committed mutations since the last baseline flush.
    pub fn delta_len(&self) -> usize {
        self.manager.delta_len()
    }

    /// Anchor check against an explicit live size (see
    /// [`IdManager::should_anchor_baseline_for_live`]).
    pub fn should_anchor_baseline_for_live(&self, live: usize) -> bool {
        self.manager.should_anchor_baseline_for_live(live)
    }

    /// Cumulative free-stack reuses (see [`IdManager::reuse_count`]).
    pub fn reuse_count(&self) -> u64 {
        self.manager.reuse_count()
    }

    /// Compaction-moved-rows flag without consuming it (see
    /// [`IdManager::baseline_invalidated`]).
    pub fn baseline_invalidated(&self) -> bool {
        self.manager.baseline_invalidated()
    }

    /// Current free-stack depth (see [`IdManager::free_depth`]).
    pub fn free_depth(&self) -> usize {
        self.manager.free_depth()
    }

    /// Index-level hole ratio (see [`IdManager::hole_ratio`]).
    pub fn hole_ratio(&self) -> f64 {
        self.manager.hole_ratio()
    }

    /// Per-component memory accounting (see
    /// [`IdManager::memory_breakdown`]).
    pub fn memory_breakdown(&self) -> IdIndexMemoryBreakdown {
        self.manager.memory_breakdown()
    }

    /// Drop the since-baseline delta (baseline-flush path only).
    pub fn clear_index_delta(&self) {
        self.manager.clear_index_delta()
    }

    /// Serialize the since-baseline delta for `id_indexer.delta`.
    pub fn serialize_delta(&self) -> Vec<u8> {
        self.manager.serialize_delta()
    }

    /// Decode delta entries from `id_indexer.delta` bytes.
    pub fn deserialize_delta(data: &[u8]) -> StorageResult<Vec<(u8, u32, IdKey)>> {
        IdManager::deserialize_delta(data)
    }

    /// Apply decoded delta entries onto the loaded baseline.
    pub fn apply_delta_entries(&self, entries: &[(u8, u32, IdKey)]) -> StorageResult<()> {
        self.manager.apply_delta_entries(entries)
    }

    pub fn iter(&self) -> Vec<(IdKey, u32)> {
        self.manager.iter()
    }

    /// Index-level live IDs in ascending order, without timestamp filtering.
    ///
    /// Deletions move here only when the key is removed (watermark-gated GC
    /// or explicit index removal). Timestamp-only deletes keep the key until
    /// GC, and lazy-reused slots rejoin on insert. Snapshot visibility must
    /// be checked separately via the row timestamps.
    pub fn live_ids(&self) -> Vec<u32> {
        self.manager.live_ids()
    }

    pub fn memory_size(&self) -> usize {
        self.manager.memory_size() + std::mem::size_of::<Self>()
    }

    pub fn compact(&self) -> StorageResult<HashMap<u32, u32>> {
        self.manager.compact()
    }

    /// Pure dense-mapping preview without mutating the index. The
    /// compaction coordinator computes this before building any
    /// replacement structures so failures leave all state untouched.
    pub fn compute_compact_mapping(&self) -> HashMap<u32, u32> {
        self.manager.compute_compact_mapping()
    }

    /// Serialize the index to bytes for persistence.
    pub fn serialize(&self) -> Vec<u8> {
        self.manager.serialize()
    }

    /// Byte snapshot for compaction journaling: captured before any remap
    /// mutation so a mid-compaction failure can restore the index exactly.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.serialize()
    }

    /// Restore a snapshot captured by [`Self::snapshot_bytes`], discarding
    /// any partial remap applied since. Used only for compaction rollback.
    /// All stripes in index order, then the core.
    pub fn restore_snapshot(&self, bytes: &[u8]) -> StorageResult<()> {
        let fresh = IdManager::deserialize(bytes)?;
        let mut guards: Vec<_> = self.manager.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.manager.core.lock();
        let fresh_guards: Vec<_> = fresh.stripes.iter().map(|s| s.lock()).collect();
        let fresh_core = fresh.core.lock();
        for (dst, src) in guards.iter_mut().zip(fresh_guards.iter()) {
            **dst = (**src).clone();
        }
        *core = SharedCore {
            keys: fresh_core.keys.clone(),
            live_ids: fresh_core.live_ids.clone(),
            free_ids: fresh_core.free_ids.clone(),
            reuse_count: fresh_core.reuse_count,
            delta_log: fresh_core.delta_log.clone(),
            baseline_invalidated: fresh_core.baseline_invalidated,
            config: fresh_core.config.clone(),
        };
        Ok(())
    }

    /// Deserialize from bytes, rebuilding the index.
    pub fn deserialize(data: &[u8]) -> StorageResult<Self> {
        let manager = IdManager::deserialize(data)?;
        Ok(Self {
            manager: Arc::new(manager),
        })
    }
}

impl Default for IdIndexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        assert_eq!(idx1, 0);

        let idx2 = indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();
        assert_eq!(idx2, 1);

        assert_eq!(
            indexer.get_index(&IdKey::Text("vertex1".to_string())),
            Some(0)
        );
        assert_eq!(
            indexer.get_index(&IdKey::Text("vertex2".to_string())),
            Some(1)
        );
        assert_eq!(indexer.get_index(&IdKey::Text("vertex3".to_string())), None);

        assert_eq!(indexer.get_key(0), Some(IdKey::Text("vertex1".to_string())));
        assert_eq!(indexer.get_key(1), Some(IdKey::Text("vertex2".to_string())));
    }

    #[test]
    fn test_int_id_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
        assert_eq!(idx1, 0);

        let idx2 = indexer.insert(IdKey::Int(200)).unwrap();
        assert_eq!(idx2, 1);

        assert_eq!(indexer.get_index(&IdKey::Int(100)), Some(0));
        assert_eq!(indexer.get_index(&IdKey::Int(200)), Some(1));
        assert_eq!(indexer.get_index(&IdKey::Int(300)), None);

        assert_eq!(indexer.get_key(0), Some(IdKey::Int(100)));
        assert_eq!(indexer.get_key(1), Some(IdKey::Int(200)));
    }

    #[test]
    fn test_mixed_id_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
        let idx2 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        let idx3 = indexer.insert(IdKey::Int(200)).unwrap();

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 2);

        assert_eq!(indexer.len(), 3);
    }

    #[test]
    fn test_live_ids_skip_deleted_gaps_in_stable_order() {
        let indexer = IdIndexer::new();
        for value in 0..10 {
            indexer
                .insert(IdKey::Int(value))
                .expect("insert must succeed");
        }
        indexer.remove(&IdKey::Int(1));
        indexer.remove(&IdKey::Int(5));
        indexer.remove(&IdKey::Int(8));

        assert_eq!(indexer.live_ids(), vec![0, 2, 3, 4, 6, 7, 9]);
    }

    #[test]
    fn test_dynamic_expansion() {
        let indexer = IdIndexer::with_config(IdIndexerConfig {
            initial_capacity: 2,
            growth_factor: 2.0,
            max_capacity: MAX_CAPACITY,
        });

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v4".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v5".to_string())).is_ok());

        assert_eq!(indexer.len(), 5);
    }

    #[test]
    fn test_duplicate_insert() {
        let indexer = IdIndexer::new();

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_err());
    }

    #[test]
    fn test_max_capacity() {
        let indexer = IdIndexer::with_config(IdIndexerConfig {
            initial_capacity: 2,
            growth_factor: DEFAULT_GROWTH_FACTOR,
            max_capacity: 3,
        });

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v4".to_string())).is_err());
    }

    #[test]
    fn test_concurrent_parallel_inserts() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let indexer = StdArc::new(IdIndexer::new());
        let mut handles = vec![];

        for thread_id in 0..4 {
            let indexer_clone = StdArc::clone(&indexer);
            let handle = thread::spawn(move || {
                for i in 0..25 {
                    let key = IdKey::Text(format!("v_{}_{}", thread_id, i));
                    let _ = indexer_clone.insert(key);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        assert_eq!(indexer.len(), 100);
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let indexer = StdArc::new(IdIndexer::new());

        for i in 0..10 {
            let key = IdKey::Text(format!("v{}", i));
            let _ = indexer.insert(key);
        }

        let mut handles = vec![];

        for _ in 0..2 {
            let indexer_clone = StdArc::clone(&indexer);
            let handle = thread::spawn(move || {
                for i in 0..10 {
                    let key = IdKey::Text(format!("v{}", i));
                    let _ = indexer_clone.get_index(&key);
                }
            });
            handles.push(handle);
        }

        let indexer_clone = StdArc::clone(&indexer);
        let handle = thread::spawn(move || {
            for i in 10..20 {
                let key = IdKey::Text(format!("v{}", i));
                let _ = indexer_clone.insert(key);
            }
        });
        handles.push(handle);

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        assert_eq!(indexer.len(), 20);
    }

    #[test]
    fn test_remove() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Text("v1".to_string())).unwrap();
        indexer.insert(IdKey::Text("v2".to_string())).unwrap();
        indexer.insert(IdKey::Text("v3".to_string())).unwrap();

        assert_eq!(indexer.len(), 3);

        indexer.remove(&IdKey::Text("v2".to_string()));
        assert_eq!(indexer.len(), 2);

        assert_eq!(indexer.get_index(&IdKey::Text("v2".to_string())), None);
    }

    #[test]
    fn test_serialize_deserialize_string_ids() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();
        indexer.insert(IdKey::Text("vertex3".to_string())).unwrap();

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 3);
        assert_eq!(
            deserialized.get_index(&IdKey::Text("vertex1".to_string())),
            Some(0)
        );
        assert_eq!(
            deserialized.get_index(&IdKey::Text("vertex2".to_string())),
            Some(1)
        );
        assert_eq!(
            deserialized.get_index(&IdKey::Text("vertex3".to_string())),
            Some(2)
        );
        assert_eq!(
            deserialized.get_key(0),
            Some(IdKey::Text("vertex1".to_string()))
        );
    }

    #[test]
    fn test_serialize_deserialize_int_ids() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Int(100)).unwrap();
        indexer.insert(IdKey::Int(200)).unwrap();
        indexer.insert(IdKey::Int(300)).unwrap();

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 3);
        assert_eq!(deserialized.get_index(&IdKey::Int(100)), Some(0));
        assert_eq!(deserialized.get_index(&IdKey::Int(200)), Some(1));
        assert_eq!(deserialized.get_index(&IdKey::Int(300)), Some(2));
        assert_eq!(deserialized.get_key(0), Some(IdKey::Int(100)));
    }

    #[test]
    fn test_serialize_deserialize_mixed_ids() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Int(100)).unwrap();
        indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        indexer.insert(IdKey::Int(200)).unwrap();
        indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 4);
        assert_eq!(deserialized.get_index(&IdKey::Int(100)), Some(0));
        assert_eq!(
            deserialized.get_index(&IdKey::Text("vertex1".to_string())),
            Some(1)
        );
        assert_eq!(deserialized.get_index(&IdKey::Int(200)), Some(2));
        assert_eq!(
            deserialized.get_index(&IdKey::Text("vertex2".to_string())),
            Some(3)
        );
    }

    #[test]
    fn test_serialize_deserialize_empty() {
        let indexer = IdIndexer::new();

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 0);
    }

    #[test]
    fn test_serialize_deserialize_with_deletions() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Int(1)).unwrap();
        indexer.insert(IdKey::Int(2)).unwrap();
        indexer.insert(IdKey::Int(3)).unwrap();
        indexer.insert(IdKey::Int(4)).unwrap();
        indexer.insert(IdKey::Int(5)).unwrap();

        indexer.remove(&IdKey::Int(2));
        indexer.remove(&IdKey::Int(4));

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 3);
        assert_eq!(deserialized.get_index(&IdKey::Int(1)), Some(0));
        assert_eq!(deserialized.get_index(&IdKey::Int(2)), None);
        assert_eq!(deserialized.get_index(&IdKey::Int(3)), Some(2));
        assert_eq!(deserialized.get_index(&IdKey::Int(4)), None);
        assert_eq!(deserialized.get_index(&IdKey::Int(5)), Some(4));
    }

    #[test]
    fn test_serialize_deserialize_large_dataset() {
        let indexer = IdIndexer::new();

        for i in 0..1000 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }

        let data = indexer.serialize();
        let deserialized = IdIndexer::deserialize(&data).unwrap();

        assert_eq!(deserialized.len(), 1000);

        for i in 0..1000 {
            assert_eq!(deserialized.get_index(&IdKey::Int(i)), Some(i as u32));
        }
    }

    #[test]
    fn test_compute_mapping_is_pure_and_matches_compact() {
        let indexer = IdIndexer::new();
        for i in 0..5 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        indexer.remove(&IdKey::Int(1));
        indexer.remove(&IdKey::Int(3));

        let preview = indexer.compute_compact_mapping();
        let mut expected = HashMap::new();
        expected.insert(2u32, 1u32);
        expected.insert(4u32, 2u32);
        assert_eq!(preview, expected);
        // Preview mutated nothing: live set and lookups are unchanged.
        assert_eq!(indexer.live_ids(), vec![0, 2, 4]);
        assert_eq!(indexer.get_index(&IdKey::Int(4)), Some(4));

        let applied = indexer.compact().unwrap();
        assert_eq!(applied, expected);
    }

    #[test]
    fn test_free_stack_reuses_hole_without_moving_survivors() {
        let indexer = IdIndexer::new();
        for i in 0..3 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        indexer.remove(&IdKey::Int(1));
        let reused = indexer.insert(IdKey::Int(99)).unwrap();
        assert_eq!(reused, 1);
        assert_eq!(indexer.get_index(&IdKey::Int(0)), Some(0));
        assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(2));
        assert_eq!(indexer.get_index(&IdKey::Int(99)), Some(1));
    }
    #[test]
    fn test_snapshot_restore_rolls_back_partial_remap() {
        let indexer = IdIndexer::new();
        for i in 0..3 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        let snapshot = indexer.snapshot_bytes();
        indexer.remove(&IdKey::Int(0));
        indexer.compact().unwrap();
        assert_eq!(indexer.get_index(&IdKey::Int(1)), Some(0));
        indexer.restore_snapshot(&snapshot).unwrap();
        assert_eq!(indexer.get_index(&IdKey::Int(0)), Some(0));
        assert_eq!(indexer.get_index(&IdKey::Int(1)), Some(1));
        assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(2));
    }

    #[test]
    fn test_lookup_collapses_index_and_visibility_check() {
        let indexer = IdIndexer::new();
        indexer.insert(IdKey::Text("a".to_string())).unwrap();
        assert_eq!(
            indexer.lookup(&IdKey::Text("a".to_string()), |_| true),
            PkLookup::Visible(0)
        );
        assert_eq!(
            indexer.lookup(&IdKey::Text("a".to_string()), |_| false),
            PkLookup::Missing
        );
        assert_eq!(
            indexer.lookup(&IdKey::Text("nope".to_string()), |_| true),
            PkLookup::Missing
        );
    }

    #[test]
    fn test_delta_roundtrip_restores_exact_ids() {
        let indexer = IdIndexer::new();
        for i in 0..5 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        indexer.remove(&IdKey::Int(1));
        indexer.remove(&IdKey::Int(3));

        let baseline = indexer.serialize();
        let delta = indexer.serialize_delta();
        let entries = IdIndexer::deserialize_delta(&delta).unwrap();
        assert_eq!(entries.len(), indexer.delta_len());

        // Replay onto an empty loader plus the baseline snapshot.
        let restored_base = IdIndexer::deserialize(&baseline).unwrap();
        assert_eq!(restored_base.len(), 3);
        let mut replay = IdManager::new();
        replay.apply_delta_entries(&entries).unwrap();
        for (key, id) in indexer.iter() {
            assert_eq!(replay.get_id(&key), Some(id));
        }
        assert_eq!(replay.len(), 3);

        // Corrupt bytes fail the whole delta, never partially.
        let mut corrupt = delta.clone();
        corrupt[4] ^= 0xff;
        assert!(IdIndexer::deserialize_delta(&corrupt).is_err());
    }

    #[test]
    fn test_compact_drops_delta_and_invalidates_baseline() {
        let indexer = IdIndexer::new();
        for i in 0..4 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        assert!(indexer.delta_len() > 0);
        indexer.remove(&IdKey::Int(0));
        indexer.remove(&IdKey::Int(1));
        indexer.compact().unwrap();
        assert_eq!(indexer.delta_len(), 0);
        assert!(indexer.should_anchor_baseline_for_live(indexer.len()));
        assert!(!indexer.should_anchor_baseline_for_live(indexer.len()));
    }

    #[test]
    fn test_indexer_rejects_oversized_text_and_negative_int() {
        let indexer = IdIndexer::new();
        let oversized = "k".repeat(graphdb_core::types::VERTEX_ID_MAX_SIZE + 1);
        assert!(indexer.insert(IdKey::Text(oversized)).is_err());
        assert!(indexer.insert(IdKey::Int(-1)).is_err());
        assert_eq!(indexer.len(), 0);
    }

    #[test]
    fn test_deserialize_rejects_impossible_count() {
        let mut corrupt = 0x0100_0000u32.to_le_bytes().to_vec();
        corrupt.extend_from_slice(&[0u8; 8]);
        assert!(IdIndexer::deserialize(&corrupt).is_err());
    }

    #[test]
    fn test_delta_rejects_impossible_count() {
        let mut corrupt = u32::MAX.to_le_bytes().to_vec();
        corrupt.extend_from_slice(&[0u8; 8]);
        assert!(IdIndexer::deserialize_delta(&corrupt).is_err());
    }

    #[test]
    fn test_delta_apply_rejects_divergence() {
        let mut base = IdManager::new();
        base.apply_delta_entries(&[(0, 7, IdKey::Int(3))]).unwrap();
        let divergent = vec![(0u8, 9u32, IdKey::Int(3))];
        assert!(base.apply_delta_entries(&divergent).is_err());
        let idempotent = vec![(0u8, 7u32, IdKey::Int(3))];
        assert!(base.apply_delta_entries(&idempotent).is_ok());
    }

    #[test]
    fn test_over_threshold_delta_forces_anchor() {
        let indexer = IdIndexer::new();
        assert!(!indexer.should_anchor_baseline_for_live(indexer.len()));
        for i in 0..PK_DELTA_ANCHOR_THRESHOLD as i64 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        assert!(indexer.should_anchor_baseline_for_live(indexer.len()));
    }

    #[test]
    fn test_reserve_is_invisible_until_registered() {
        let indexer = IdIndexer::new();
        let id = indexer.reserve_next().unwrap();
        assert_eq!(id, 0);
        assert_eq!(indexer.len(), 0);
        assert!(indexer.live_ids().is_empty());
        assert_eq!(indexer.delta_len(), 0);
        assert_eq!(indexer.get_key(id), None);
        // The high-water mark grows so concurrent inserts skip the slot.
        let other = indexer.insert(IdKey::Int(7)).unwrap();
        assert_eq!(other, 1);
        indexer.register_reserved(IdKey::Int(7), 0).unwrap_err();
        indexer.register_reserved(IdKey::Int(5), id).unwrap();
        assert_eq!(indexer.get_index(&IdKey::Int(5)), Some(0));
        assert_eq!(indexer.live_ids(), vec![0, 1]);
        assert_eq!(indexer.delta_len(), 2);
    }

    #[test]
    fn test_reserve_reuses_free_stack_hole() {
        let indexer = IdIndexer::new();
        indexer.insert(IdKey::Int(0)).unwrap();
        let hole = indexer.insert(IdKey::Int(1)).unwrap();
        indexer.remove(&IdKey::Int(1));
        let reserved = indexer.reserve_next().unwrap();
        assert_eq!(reserved, hole);
        indexer.register_reserved(IdKey::Int(2), reserved).unwrap();
        assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(hole));
    }

    #[test]
    fn test_register_reserved_rejects_bound_or_foreign_slot() {
        let indexer = IdIndexer::new();
        let bound = indexer.insert(IdKey::Int(0)).unwrap();
        assert!(indexer.register_reserved(IdKey::Int(9), bound).is_err());
        assert!(indexer.register_reserved(IdKey::Int(0), 4).is_err());
        // A key already bound cannot claim any slot.
        let free = indexer.reserve_next().unwrap();
        assert!(indexer.register_reserved(IdKey::Int(0), free).is_err());
        assert_eq!(indexer.get_key(free), None);
    }

    #[test]
    fn test_release_reserved_returns_only_unbound_slots() {
        let indexer = IdIndexer::new();
        let reserved = indexer.reserve_next().unwrap();
        let bound = indexer.insert(IdKey::Int(1)).unwrap();
        // Releasing a bound id is a no-op; releasing the reservation frees it.
        indexer.release_reserved(bound);
        indexer.release_reserved(reserved);
        let reused = indexer.reserve_next().unwrap();
        assert_eq!(reused, reserved);
    }

    #[test]
    fn test_memory_breakdown_sums_to_usage() {
        let indexer = IdIndexer::new();
        for i in 0..8 {
            indexer.insert(IdKey::Text(format!("vertex-{i}"))).unwrap();
        }
        indexer.remove(&IdKey::Text("vertex-0".to_string()));
        let breakdown = indexer.memory_breakdown();
        assert_eq!(breakdown.live_count, 7);
        assert_eq!(breakdown.free_depth, 1);
        assert_eq!(breakdown.slot_count, 8);
        assert_eq!(breakdown.delta_entries, indexer.delta_len());
        assert!(breakdown.keys_heap_bytes > 0);
        assert!(breakdown.delta_heap_bytes > 0);
        assert!(breakdown.map_bytes > 0);
        assert!(breakdown.set_bytes > 0);
        assert!(
            breakdown.total_bytes
                >= breakdown.keys_heap_bytes
                    + breakdown.delta_heap_bytes
                    + breakdown.map_bytes
                    + breakdown.set_bytes
                    + breakdown.free_bytes
        );
        assert_eq!(breakdown.total_bytes, indexer.manager.memory_usage());
    }

    #[test]
    fn test_reuse_count_tracks_free_stack_pops() {
        let indexer = IdIndexer::new();
        indexer.insert(IdKey::Int(0)).unwrap();
        indexer.insert(IdKey::Int(1)).unwrap();
        assert_eq!(indexer.reuse_count(), 0);
        indexer.remove(&IdKey::Int(0));
        indexer.insert(IdKey::Int(2)).unwrap();
        assert_eq!(indexer.reuse_count(), 1);
        assert_eq!(indexer.free_depth(), 0);
        indexer.insert(IdKey::Int(3)).unwrap();
        assert_eq!(indexer.reuse_count(), 1);
    }

    #[test]
    fn test_reuse_claims_largest_hole_first() {
        let indexer = IdIndexer::new();
        for i in 0..4 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        // Free in ascending order so stack order alone would reclaim the
        // smallest hole first; ordered reuse must refill the tail instead.
        indexer.remove(&IdKey::Int(1));
        indexer.remove(&IdKey::Int(3));
        assert!((indexer.hole_ratio() - 0.5).abs() < f64::EPSILON);
        let first = indexer.insert(IdKey::Int(10)).unwrap();
        assert_eq!(first, 3);
        let second = indexer.insert(IdKey::Int(11)).unwrap();
        assert_eq!(second, 1);
        assert_eq!(indexer.free_depth(), 0);
        assert_eq!(indexer.hole_ratio(), 0.0);
    }

    #[test]
    fn test_anchor_threshold_scales_with_live_size() {
        assert_eq!(
            IdManager::anchor_threshold_for_live(0),
            PK_DELTA_ANCHOR_THRESHOLD
        );
        assert_eq!(
            IdManager::anchor_threshold_for_live(PK_DELTA_ANCHOR_THRESHOLD * 8),
            PK_DELTA_ANCHOR_THRESHOLD * 2
        );
        let indexer = IdIndexer::new();
        for i in 0..8 {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        indexer.clear_index_delta();
        assert!(!indexer.should_anchor_baseline_for_live(8));
        for i in 8..(8 + PK_DELTA_ANCHOR_THRESHOLD as i64) {
            indexer.insert(IdKey::Int(i)).unwrap();
        }
        assert!(indexer.should_anchor_baseline_for_live(8));
    }

    #[test]
    fn test_try_reclaim_cancels_a_release() {
        let indexer = IdIndexer::new();
        let reserved = indexer.reserve_next().unwrap();
        indexer.release_reserved(reserved);
        assert!(indexer.try_reclaim(reserved));
        // Reclaimed: the slot is held out of the free stack again, so the
        // next reservation grows elsewhere.
        let fresh = indexer.reserve_next().unwrap();
        assert_ne!(fresh, reserved);
        // A bound slot cannot be reclaimed.
        let bound = indexer.insert(IdKey::Int(1)).unwrap();
        assert!(!indexer.try_reclaim(bound));
        // An unbound slot that was never released reclaims as a no-op.
        let hole = indexer.reserve_next().unwrap();
        indexer.release_reserved(hole);
        indexer.reserve_next().unwrap();
        assert!(!indexer.try_reclaim(hole));
    }
}
