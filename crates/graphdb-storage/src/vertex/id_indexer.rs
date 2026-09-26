//! Shared primary-key index (see the parent `vertex` module docs).
//!
//! Module layout:
//! - [`config`] - construction parameters
//! - [`key`] - the external `IdKey` union and its shape validation
//! - [`lookup`] - the visibility-aware `PkLookup` result
//! - [`manager`] - `IdManager`: striped maps plus the shared allocation core
//! - [`memory`] - per-component memory accounting
//! - [`codec`] - baseline snapshot and delta persistence codecs
//! - [`tests`] - unit tests for the whole module

mod codec;
mod config;
mod key;
mod lookup;
pub mod manager;
mod memory;

pub use config::IdIndexerConfig;
pub use key::IdKey;
pub use lookup::PkLookup;
pub use manager::{stripe_index, IdManager};
pub use memory::IdIndexMemoryBreakdown;

/// Since-baseline delta entries beyond this count force the next
/// incremental flush to anchor a new full baseline instead of extending
/// the delta file. Bounds delta replay cost, delta file size, and the
/// double-stored key memory between baselines.
pub const PK_DELTA_ANCHOR_THRESHOLD: usize = 8192;

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
    manager: std::sync::Arc<IdManager>,
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
            manager: std::sync::Arc::new(IdManager::with_config(config.clone())),
        }
    }

    pub fn insert(&self, key: IdKey) -> graphdb_core::error::StorageResult<u32> {
        self.manager.insert(key)
    }

    /// Reserve a local id without binding a key (see
    /// [`IdManager::reserve_next`]).
    pub fn reserve_next(&self) -> graphdb_core::error::StorageResult<u32> {
        self.manager.reserve_next()
    }

    /// Bind a key to a reserved id at commit apply (see
    /// [`IdManager::register_reserved`]).
    pub fn register_reserved(&self, key: IdKey, id: u32) -> graphdb_core::error::StorageResult<()> {
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
        memory::memory_breakdown(&self.manager)
    }

    /// Drop the since-baseline delta (baseline-flush path only).
    pub fn clear_index_delta(&self) {
        self.manager.clear_index_delta()
    }

    /// Serialize the since-baseline delta for `id_indexer.delta`.
    pub fn serialize_delta(&self) -> Vec<u8> {
        let core = self.manager.core.lock();
        codec::serialize_delta(&core)
    }

    /// Decode delta entries from `id_indexer.delta` bytes.
    pub fn deserialize_delta(
        data: &[u8],
    ) -> graphdb_core::error::StorageResult<Vec<(u8, u32, IdKey)>> {
        codec::deserialize_delta(data)
    }

    /// Apply decoded delta entries onto the loaded baseline without
    /// recording them again (replay must not extend the live delta log).
    /// Insert restores the exact baseline id; remove drops the key and
    /// recycles its id. All stripes in index order, then the core.
    pub fn apply_delta_entries(
        &self,
        entries: &[(u8, u32, IdKey)],
    ) -> graphdb_core::error::StorageResult<()> {
        let mut guards: Vec<_> = self.manager.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.manager.core.lock();
        for (op, id, key) in entries {
            match op {
                0 => {
                    key::validate_key_shape(key)?;
                    let stripe = stripe_index(key);
                    if let Some(existing) = guards[stripe].get(key).copied() {
                        if existing != *id {
                            return Err(graphdb_core::error::StorageError::deserialize_error(
                                format!(
                                    "pk delta insert diverges for {:?}: baseline {} vs delta {}",
                                    key, existing, id
                                ),
                            ));
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
                    return Err(graphdb_core::error::StorageError::deserialize_error(
                        format!("pk delta unknown op {}", op),
                    ));
                }
            }
        }
        Ok(())
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

    /// All stripes in index order, then the core. Exclusive path only
    /// (compaction runs barriered); point probes block while it holds the
    /// segments.
    pub fn compact(
        &self,
    ) -> graphdb_core::error::StorageResult<std::collections::HashMap<u32, u32>> {
        let mapping = self.compute_compact_mapping();
        if mapping.is_empty() {
            // Already dense; still clear free list if it had stale entries.
            // Ids did not move, so the since-baseline delta stays valid.
            self.manager.core.lock().free_ids.clear();
            return Ok(std::collections::HashMap::new());
        }

        let mut guards: Vec<_> = self.manager.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.manager.core.lock();
        let mut entries: Vec<(u32, IdKey)> = Vec::new();
        for map in guards.iter() {
            entries.extend(map.iter().map(|(key, &idx)| (idx, key.clone())));
        }
        entries.sort_by_key(|(old_id, _)| *old_id);
        rebuild_with_mapping_locked(&mut core, &mut guards, &entries)?;
        core.free_ids.clear();
        // Live rows moved: delta entries addressed by old ids are stale.
        // Drop them and force the next flush to anchor a new baseline.
        core.delta_log.clear();
        core.baseline_invalidated = true;

        Ok(mapping)
    }

    /// Pure dense-mapping preview without mutating the index. The
    /// compaction coordinator computes this before building any
    /// replacement structures so failures leave all state untouched.
    pub fn compute_compact_mapping(&self) -> std::collections::HashMap<u32, u32> {
        self.manager.compute_compact_mapping()
    }

    /// Serialize the index to bytes for persistence.
    pub fn serialize(&self) -> Vec<u8> {
        let core = self.manager.core.lock();
        codec::serialize(&core)
    }

    /// Byte snapshot for compaction journaling: captured before any remap
    /// mutation so a mid-compaction failure can restore the index exactly.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.serialize()
    }

    /// Restore a snapshot captured by [`Self::snapshot_bytes`], discarding
    /// any partial remap applied since. Used only for compaction rollback.
    /// All stripes in index order, then the core.
    pub fn restore_snapshot(&self, bytes: &[u8]) -> graphdb_core::error::StorageResult<()> {
        let fresh = IdManager::deserialize(bytes)?;
        let mut guards: Vec<_> = self.manager.stripes.iter().map(|s| s.lock()).collect();
        let mut core = self.manager.core.lock();
        let fresh_guards: Vec<_> = fresh.stripes.iter().map(|s| s.lock()).collect();
        let fresh_core = fresh.core.lock();
        for (dst, src) in guards.iter_mut().zip(fresh_guards.iter()) {
            **dst = (**src).clone();
        }
        core.clone_from_fresh(&fresh_core);
        Ok(())
    }

    /// Deserialize from bytes, rebuilding the index.
    pub fn deserialize(data: &[u8]) -> graphdb_core::error::StorageResult<Self> {
        let manager = IdManager::deserialize(data)?;
        Ok(Self {
            manager: std::sync::Arc::new(manager),
        })
    }
}

impl Default for IdIndexer {
    fn default() -> Self {
        Self::new()
    }
}

fn rebuild_with_mapping_locked(
    core: &mut manager::SharedCore,
    guards: &mut [parking_lot::MutexGuard<'_, std::collections::HashMap<IdKey, u32>>],
    entries: &[(u32, IdKey)],
) -> graphdb_core::error::StorageResult<()> {
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

#[cfg(test)]
mod tests;
