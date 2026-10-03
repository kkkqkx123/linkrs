use std::sync::atomic::Ordering;

use graphdb_core::{DataType, StorageResult, Value};

use crate::encoding::ColumnEncoding;

use super::chunk::ColumnChunk;
use super::chunk_residency::{ChunkResidency, EvictedSnapshot};
use super::{BufferLedger, Column, EVICTION_SEGMENT_BYTES};

impl Column {
    /// Whether the chunk may be evicted: resident, encoded, with no
    /// unmerged overlay writes and no live version-chain entries. Version
    /// chains live in the segment state itself, so the chunk check is
    /// complete; callers recheck inside [`Self::evict_chunk`].
    pub fn chunk_evictable(&self, chunk_idx: usize) -> bool {
        let chunks = self.chunks.read();
        chunks
            .get(chunk_idx)
            .is_some_and(|chunk| chunk.is_evictable())
    }

    /// Synchronously load an evicted chunk and mark it hot. Returns whether
    /// a load happened. Load failures propagate as storage errors.
    ///
    /// Promotion preserves the chunk's MVCC side state (visibility,
    /// chains, zone, dirty marks, overflow mappings): only the payload
    /// (raw, encoding, overlay, residency) is rebuilt. Overlay entries that
    /// landed while the snapshot decoded win over snapshot values.
    pub fn ensure_resident(&self, chunk_idx: usize) -> StorageResult<bool> {
        let snapshot = {
            let chunks = self.chunks.read();
            match chunks.get(chunk_idx) {
                Some(chunk) => match &chunk.read_state().residency {
                    ChunkResidency::Resident => return Ok(false),
                    ChunkResidency::Evicted(snapshot) => snapshot.clone(),
                },
                None => return Ok(false),
            }
        };
        let pairs = snapshot.decode_all()?;
        // Snapshot values keyed by absolute row; overlay entries collected
        // under the write latch override them below.
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return Ok(false);
        };
        let (row_offset, row_count) = (chunk.row_offset, chunk.row_count);
        let data_type = self.data_type.clone();
        let nullable = self.nullable;
        // Overflow rows decode as placeholders for the re-encode below,
        // mirroring the encode-time base; payloads stay in the sidecar.
        let overflow_rows: std::collections::HashSet<u32> = {
            let state = chunk.read_state();
            state.overflow_rows.keys().copied().collect()
        };
        let fresh = ColumnChunk::new(row_offset, row_count, &data_type, nullable);
        fresh.touch();
        {
            let mut fresh_state = fresh.write_state();
            for (row, value) in &pairs {
                let local = row.saturating_sub(row_offset);
                let value = if overflow_rows.contains(&(local as u32)) {
                    match data_type {
                        DataType::Blob => Some(Value::Blob(Vec::new())),
                        _ => Some(Value::string("")),
                    }
                } else {
                    value.clone()
                };
                let _ = fresh_state.raw.as_storage_mut().set(local, value.as_ref());
            }
        }
        // Restore the pre-evict encoding from the same values so promotion
        // returns the chunk to its encoded form instead of a raw copy.
        // Profiles (min/max/raw size) come from the snapshot; counts and
        // the compressed size reflect the fresh encoding.
        if snapshot.encoding != crate::encoding::EncodingType::None {
            let values: Vec<Option<Value>> = pairs
                .iter()
                .map(|(row, value)| {
                    let local = row.saturating_sub(row_offset) as u32;
                    if overflow_rows.contains(&local) {
                        match data_type {
                            DataType::Blob => Some(Value::Blob(Vec::new())),
                            _ => Some(Value::string("")),
                        }
                    } else {
                        value.clone()
                    }
                })
                .collect();
            let encoded = Self::encode_slice(&values, &data_type, snapshot.encoding, 255);
            if encoded.is_encoded() {
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                let mut fresh_state = fresh.write_state();
                fresh_state.encoding = encoded;
                fresh_state.encoding_meta = snapshot.meta.clone();
                fresh_state.encoding_meta.scheme = fresh_state.encoding.encoding_type();
                fresh_state.encoding_meta.num_values = num_values;
                fresh_state.encoding_meta.all_null = num_values == 0;
                fresh_state.encoding_meta.compressed_size =
                    fresh_state.encoding.memory_usage() as u64;
            }
        }
        // Publish under the segment write latch, merging overlay entries
        // that landed during the decode so no concurrent write is lost.
        // Only the payload fields move; MVCC side state stays in place.
        let mut state = chunk.write_state();
        if state.residency.is_resident() {
            // Another promoter won the race; keep its result.
            return Ok(false);
        }
        let mut fresh_state = fresh.state.into_inner();
        for (local, value) in state.overlay.iter() {
            let _ = fresh_state
                .raw
                .as_storage_mut()
                .set(*local as usize, value.as_ref());
        }
        state.raw = fresh_state.raw;
        state.encoding = fresh_state.encoding;
        state.encoding_meta = fresh_state.encoding_meta;
        if state.overlay.len() != 0 && state.encoding.is_encoded() {
            // Overlay writes landed during the decode and were merged into
            // the raw buffers above, but the restored encoding was built
            // from snapshot values only. Drop the encoding so the merged
            // raw buffers stay authoritative; the next encode pass relearns.
            state.encoding = ColumnEncoding::None;
            state.encoding_meta = crate::encoding::ChunkEncodingMeta::default();
        }
        state.updates_since_encode = 0;
        state.overlay.clear();
        state.residency = ChunkResidency::Resident;
        Ok(true)
    }

    /// Batch miss-load: promote every evicted chunk covering `rows` once
    /// before a grouped decode, avoiding per-row page faults. Returns the
    /// number of chunks loaded. Background batch loads behind this entry are
    /// charged against the task quota in [`super::MAX_BACKGROUND_LOAD_CHUNKS`]
    /// segments.
    pub fn ensure_resident_range(&self, rows: &[usize]) -> StorageResult<usize> {
        let mut loaded = 0usize;
        let mut remaining = rows.to_vec();
        while !remaining.is_empty() {
            let (n, rest) = self
                .ensure_resident_range_with_quota(&remaining, super::MAX_BACKGROUND_LOAD_CHUNKS)?;
            if n == 0 {
                break;
            }
            loaded += n;
            remaining = rest;
        }
        Ok(loaded)
    }

    /// Promote every evicted chunk. Used by encoding passes and flush
    /// snapshots so persisted output keeps full fidelity; the source table
    /// is untouched when this runs on its flush-time clone.
    pub fn ensure_all_resident(&self) -> StorageResult<usize> {
        let mut loaded = 0usize;
        let len = self.chunks.read().len();
        for idx in 0..len {
            if self.ensure_resident(idx)? {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Release one cold chunk's decoded buffers, spilling the compressed
    /// snapshot to a spill file and retaining only row range, encoding
    /// scheme, and profiles in memory. Returns bytes released, or 0 when
    /// the chunk is not evictable. Zone maps stay resident in the segment
    /// state and keep serving; HLL stays at the column level. A spill
    /// failure reports an error so the caller skips the chunk instead of
    /// retaining heap pages.
    ///
    /// Exclusive-only (shard write lock): point writes never run
    /// concurrently, but evictability is still rechecked under the segment
    /// write latch so a racing promotion abandons the chunk for this pass.
    pub fn evict_chunk(&self, chunk_idx: usize) -> StorageResult<u64> {
        if !self.chunk_evictable(chunk_idx) {
            return Ok(0);
        }
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return Ok(0);
        };
        let (start, count, encoding, meta) = {
            let state = chunk.read_state();
            (
                chunk.row_offset,
                chunk.row_count,
                state.encoding.encoding_type(),
                state.encoding_meta.clone(),
            )
        };
        let values: Vec<Option<Value>> = (start..start.saturating_add(count))
            .map(|row| self.get_in(&chunks, row))
            .collect();
        let snapshot = EvictedSnapshot::capture(start, values, encoding, meta)?;
        let mut state = chunk.write_state();
        // Recheck under the latch: a racing promotion or point write
        // abandons this chunk for this pass.
        if state.residency.is_evicted()
            || !state.encoding.is_encoded()
            || state.overlay.len() != 0
            || state
                .version_chains
                .as_ref()
                .is_some_and(|chains| chains.values().any(|chain| !chain.is_empty()))
        {
            return Ok(0);
        }
        let released =
            (state.raw.as_storage().memory_usage() + state.encoding.memory_usage()) as u64;
        state.raw.as_storage_mut().clear();
        state.encoding = ColumnEncoding::None;
        state.residency = ChunkResidency::Evicted(snapshot);
        Ok(released)
    }

    /// Evict resident cold chunks oldest-first until `budget` bytes are
    /// released. Returns `(chunks_evicted, bytes_released)`. Per-chunk
    /// failures are skipped with a warning so one corruptible chunk never
    /// blocks the watermark pass.
    pub fn evict_cold_chunks(&self, budget: u64) -> (usize, u64) {
        let (evicted, freed, _) = self.evict_cold_chunks_with_quota(budget, u64::MAX);
        (evicted, freed)
    }

    /// Quota-segmented eviction for background tasks.
    ///
    /// `budget` is the total release target; `task_quota` caps one segment so
    /// over-quota background work proceeds in segments instead of one burst.
    /// Returns `(chunks_evicted, bytes_released, segments)`. Each segment
    /// re-selects evictable chunks oldest-first and rechecks evictability
    /// inside [`Self::evict_chunk`], so writes landing during confirmation
    /// abandon that chunk for this pass.
    pub fn evict_cold_chunks_with_quota(
        &self,
        budget: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        let chunks = self.chunks.read();
        if budget == 0 || chunks.is_empty() {
            return (0, 0, 0);
        }
        drop(chunks);
        let segment = task_quota.clamp(1, EVICTION_SEGMENT_BYTES).min(budget);
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        while freed < budget {
            let target = freed.saturating_add(segment).min(budget);
            let order: Vec<(u64, usize)> = {
                let chunks = self.chunks.read();
                let mut order: Vec<(u64, usize)> = (0..chunks.len())
                    .filter(|&idx| chunks.get(idx).is_some_and(|chunk| chunk.is_evictable()))
                    .map(|idx| (chunks[idx].last_access.load(Ordering::Relaxed), idx))
                    .collect();
                order.sort_unstable();
                order
            };
            segments += 1;
            let mut progress = false;
            for (_, idx) in order {
                // Recheck under the pass: a racing promotion or write
                // abandons the chunk via the in-evict recheck.
                if !self.chunk_evictable(idx) {
                    continue;
                }
                if freed >= target {
                    break;
                }
                match self.evict_chunk(idx) {
                    Ok(0) => {}
                    Ok(released) => {
                        count += 1;
                        freed += released;
                        progress = true;
                    }
                    Err(e) => {
                        log::warn!("chunk eviction skipped for {}[{}]: {}", self.name, idx, e);
                    }
                }
            }
            if !progress {
                break;
            }
        }
        (count, freed, segments)
    }

    /// Quota-capped batch promotion for background scans.
    ///
    /// Loads at most `max_chunks` evicted chunks covering `rows`, returning
    /// `(chunks_loaded, remaining_rows)`. Callers with a task memory quota
    /// process the loaded prefix, release pressure, then continue with the
    /// remainder instead of promoting the whole working set at once.
    pub fn ensure_resident_range_with_quota(
        &self,
        rows: &[usize],
        max_chunks: usize,
    ) -> StorageResult<(usize, Vec<usize>)> {
        let mut idxs: Vec<usize> = rows
            .iter()
            .filter_map(|row| self.chunk_index_for_row(*row))
            .collect();
        idxs.sort_unstable();
        idxs.dedup();
        let mut loaded = 0usize;
        let mut done_through = 0usize;
        for (position, idx) in idxs.iter().enumerate() {
            if loaded >= max_chunks {
                break;
            }
            if self.ensure_resident(*idx)? {
                loaded += 1;
            }
            done_through = position + 1;
        }
        let remaining: Vec<usize> = if done_through >= idxs.len() {
            Vec::new()
        } else {
            let pending: std::collections::HashSet<usize> =
                idxs[done_through..].iter().copied().collect();
            rows.iter()
                .copied()
                .filter(|row| {
                    self.chunk_index_for_row(*row)
                        .is_some_and(|idx| pending.contains(&idx))
                })
                .collect()
        };
        Ok((loaded, remaining))
    }

    /// Unified buffer ledger for this column in one pass: resident bytes
    /// (decoded heap including the overflow side store), retained
    /// eviction-snapshot bytes, the overflow subset, dirty pages and chunk
    /// counts. One chunks read plus one overflow lock; eviction quotas
    /// and observability share these totals instead of three separate
    /// tallies.
    pub fn buffer_ledger(&self) -> BufferLedger {
        let chunks = self.chunks.read();
        let mut resident_chunks = 0usize;
        let mut evicted_chunks = 0usize;
        let mut evicted_bytes = 0usize;
        let mut dirty_pages = 0usize;
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            if state.residency.is_resident() {
                resident_chunks += 1;
            } else {
                evicted_chunks += 1;
            }
            if let Some(snapshot) = state.residency.evicted_snapshot() {
                evicted_bytes += snapshot.compressed_bytes();
            }
            dirty_pages += state.dirty_pages.len();
        }
        let overflow_bytes = self.overflow_store.lock().memory_usage();
        let resident_bytes = self.memory_usage().saturating_sub(evicted_bytes);
        BufferLedger {
            resident_bytes,
            evicted_bytes,
            overflow_bytes,
            dirty_pages,
            resident_chunks,
            evicted_chunks,
        }
    }

    /// Buffered overwrite entries in every chunk of this column.
    pub fn overlay_entry_count(&self) -> usize {
        let chunks = self.chunks.read();
        let mut total = 0usize;
        for chunk in chunks.iter() {
            total += chunk.read_state().overlay.len();
        }
        total
    }

    /// Chunk indexes whose overlay load makes them recode candidates.
    pub fn pending_recode_chunks(&self) -> Vec<usize> {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.needs_recode())
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Early merge for one hot chunk, before its overlay fills.
    ///
    /// Raw chunks merge cheaply: overlay values are written back to the raw
    /// buffer and the overlay clears, so later writes stay in place.
    /// Encoded chunks are only marked hot here; the actual re-encode stays
    /// with the flush path, which has the full chunk profile. Returns
    /// whether any merge or hot-marking happened. Never fails the write:
    /// merge errors leave the overlay intact.
    ///
    /// The overlay is collected before merging so the segment latch is never
    /// held across the raw write path (container-before-member order).
    pub fn maybe_merge_hot_chunk(&self, chunk_idx: usize) -> bool {
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return false;
        };
        let (overlay_len, updates, encoded, evicted, half_budget, row_offset) = {
            let state = chunk.read_state();
            (
                state.overlay.len(),
                state.updates_since_encode,
                state.encoding.is_encoded(),
                !state.residency.is_resident(),
                (state.overlay.capacity() / 2).max(1),
                chunk.row_offset,
            )
        };
        if evicted {
            return false;
        }
        if overlay_len < half_budget && updates < half_budget as u64 {
            return false;
        }
        if !encoded {
            let merged: Vec<(u32, Option<Value>)> = chunk
                .read_state()
                .overlay
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            drop(chunks);
            let mut applied: std::collections::HashSet<u32> = std::collections::HashSet::new();
            for (local, value) in &merged {
                let row = row_offset + *local as usize;
                if self.write_raw_inner(row, value.as_ref()).is_ok() {
                    applied.insert(*local);
                }
            }
            if !applied.is_empty() {
                let chunks = self.chunks.read();
                if let Some(chunk) = chunks.get(chunk_idx) {
                    let mut state = chunk.write_state();
                    // Only clear entries this pass merged: entries that
                    // landed during the merge, or whose write failed, stay
                    // for the next pass.
                    for (local, value) in &merged {
                        if !applied.contains(local) {
                            continue;
                        }
                        let same = state
                            .overlay
                            .get(*local)
                            .is_some_and(|current| &current == value);
                        if same {
                            state.overlay.remove(*local);
                        }
                    }
                    if state.overlay.len() == 0 {
                        state.updates_since_encode = 0;
                    }
                }
                return true;
            }
            return false;
        }
        drop(chunks);
        let chunks = self.chunks.read();
        if let Some(chunk) = chunks.get(chunk_idx) {
            if !chunk.needs_recode() {
                chunk.mark_hot();
                return true;
            }
        }
        false
    }
}
