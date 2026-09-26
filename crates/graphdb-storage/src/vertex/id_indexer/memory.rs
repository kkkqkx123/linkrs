//! Memory accounting for the primary-key index.

use std::collections::HashMap;

use parking_lot::MutexGuard;

use super::key::IdKey;
use super::manager::{IdManager, ID_STRIPE_COUNT};

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

/// Per-component memory accounting backing [`IdManager::memory_usage`].
/// Stripes in index order, then the core.
pub(super) fn memory_breakdown(manager: &IdManager) -> IdIndexMemoryBreakdown {
    let mut guards: Vec<MutexGuard<'_, HashMap<IdKey, u32>>> = Vec::with_capacity(ID_STRIPE_COUNT);
    for stripe in &manager.stripes {
        guards.push(stripe.lock());
    }
    let core = manager.core.lock();
    let mut keys_heap_bytes = 0usize;
    for key_opt in &core.keys {
        if let Some(IdKey::Text(text)) = key_opt {
            keys_heap_bytes += text.len();
        }
    }
    let mut delta_heap_bytes = 0usize;
    for delta in &core.delta_log {
        match delta {
            super::manager::IndexDelta::Insert { key, .. }
            | super::manager::IndexDelta::Remove { key } => {
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
