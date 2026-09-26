use graphdb_core::types::Timestamp;
use graphdb_core::Value;

use super::Column;

/// One before-image of a row's value, valid on `[start_ts, end_ts)`.
///
/// The per-row version chain stores entries ordered by `start_ts` ascending
/// (oldest first); the last entry is the most recent before-image, adjacent to
/// the current value that lives in the column storage and is valid from
/// `visibility.create_ts` onward. Each write pushes the previous current value
/// here with its lifetime.
#[derive(Debug, Clone)]
pub struct VersionEntry {
    /// First timestamp at which this version is visible.
    pub start_ts: Timestamp,
    /// One past the last timestamp at which this version is visible.
    pub end_ts: Timestamp,
    /// The before-image value; `None` denotes a null (missing) value.
    pub value: Option<Value>,
}

/// Per-row visibility metadata for MVCC isolation.
///
/// Lightweight layer that replaces per-column version chains for transaction
/// isolation purposes. Each row stores its creation timestamp; historical
/// values are stored in the optional version chains. Row deletion is tracked
/// at the table level (`VertexTimestamp` / `CsrWithProperties::visibility`),
/// so column-level visibility only needs the creation time.
#[derive(Debug, Clone, Default)]
pub struct RowVisibility {
    create_ts: Vec<Timestamp>,
}

impl RowVisibility {
    pub fn new() -> Self {
        Self {
            create_ts: Vec::new(),
        }
    }

    #[inline]
    pub fn mark_created(&mut self, row_idx: usize, ts: Timestamp) {
        self.ensure_len(row_idx + 1);
        self.create_ts[row_idx] = ts;
    }

    pub fn create_ts(&self) -> &[Timestamp] {
        &self.create_ts
    }

    pub fn ensure_len(&mut self, n: usize) {
        if self.create_ts.len() < n {
            self.create_ts.resize(n, 0);
        }
    }

    /// Keep the first `keep` entries, dropping the tail.
    pub fn truncate(&mut self, keep: usize) {
        self.create_ts.truncate(keep);
    }

    /// Split off entries at `at`, returning the tail as a new slice.
    pub fn split_off(&mut self, at: usize) -> Self {
        Self {
            create_ts: self.create_ts.split_off(at),
        }
    }

    pub fn memory_usage(&self) -> usize {
        self.create_ts.len() * std::mem::size_of::<Timestamp>()
    }
}

/// Statistics for MVCC version chains of a column.
#[derive(Debug, Clone, Copy)]
pub struct VersionChainStats {
    pub total_rows: usize,
    pub total_entries: usize,
    pub max_len: usize,
    pub avg_len: f64,
    pub memory_bytes: usize,
}

// ---------------------------------------------------------------------------
// Column MVCC methods
// ---------------------------------------------------------------------------

impl Column {
    /// Versioned write: records the current value as a before-image valid on
    /// `[create_ts, ts)`, then stores `value` as the current value valid
    /// from `ts` onward. Rows written for the first time get no before-image.
    ///
    /// Before-image capture and the value write share one segment latch so a
    /// concurrent snapshot read never observes half a versioned write.
    pub fn set_versioned(
        &self,
        row_idx: usize,
        value: Option<&Value>,
        ts: Timestamp,
    ) -> graphdb_core::StorageResult<()> {
        if value.is_none() && !self.nullable {
            return Err(graphdb_core::StorageError::null_value_not_allowed(
                self.name.clone(),
            ));
        }
        if let Some(v) = value {
            if v.is_null() && !self.nullable {
                return Err(graphdb_core::StorageError::null_value_not_allowed(
                    self.name.clone(),
                ));
            }
        }
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        // Capture the before-image inputs before taking the write latch so
        // the segment latch never nests inside a second container access.
        let (old_create, current, cur_null) = {
            let chunks = self.chunks.read();
            let capacity = self.chunk_capacity();
            let chunk = chunks.get(row_idx / capacity.max(1));
            let old_create = chunk
                .map(|chunk| {
                    if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                        chunk
                            .read_state()
                            .visibility
                            .create_ts()
                            .get(row_idx - chunk.row_offset)
                            .copied()
                            .unwrap_or(0)
                    } else {
                        0
                    }
                })
                .unwrap_or(0);
            let covered = chunk.is_some_and(|chunk| {
                row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count
            });
            let (current, cur_null) = if covered && old_create < ts {
                let current = self.get_in(&chunks, row_idx);
                let cur_null = chunk.is_some_and(|chunk| {
                    chunk
                        .read_state()
                        .raw
                        .as_storage()
                        .is_null(row_idx - chunk.row_offset)
                });
                (current, cur_null)
            } else {
                (None, false)
            };
            (old_create, current, cur_null)
        };
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            state.visibility.ensure_len(chunk.row_count);
            // Only record a before-image when the current value genuinely predates
            // this write (guards against zero-length ranges from rollback writes
            // that reuse the transaction's original timestamp).
            if local < chunk.row_count && old_create < ts && (current.is_some() || cur_null) {
                if state.version_chains.is_none() {
                    state.version_chains = Some(vec![Vec::new(); chunk.row_count]);
                }
                if let Some(chains) = state.version_chains.as_mut() {
                    if chains.len() < chunk.row_count {
                        chains.resize(chunk.row_count, Vec::new());
                    }
                    if local < chains.len() {
                        chains[local].push(VersionEntry {
                            start_ts: old_create,
                            end_ts: ts,
                            value: current.clone(),
                        });
                    }
                }
            }
            let absorbed = self.write_core(chunk, state, row_idx, value, use_chunk_layer)?;
            state.visibility.mark_created(local, ts);
            Ok(absorbed)
        })?;
        self.observe_write(row_idx, value);
        self.mark_dirty(row_idx);
        if absorbed {
            if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
                let _ = self.maybe_merge_hot_chunk(chunk_idx);
            }
        }
        Ok(())
    }

    /// Read the value visible at `query_ts` for a row.
    ///
    /// Returns the current value when it was written at or before `query_ts`;
    /// otherwise searches the version chain for the before-image covering
    /// `query_ts`. A `None` return means the value is null at `query_ts`.
    /// Uses the unified `Visibility` helper so column and edge layers share the
    /// same snapshot visibility semantics.
    ///
    /// Column visibility only: this says nothing about row liveness. A row
    /// deleted at or before `query_ts` still returns its last column value
    /// here. Every caller must filter through the row-liveness layer
    /// (`VertexTimestamp::is_valid` or the table scan) and never serve this
    /// result directly.
    pub fn get_at_ts(&self, row_idx: usize, query_ts: Timestamp) -> Option<Value> {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        let chunk = chunks.get(row_idx / capacity.max(1))?;
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return None;
        }
        let local = row_idx - chunk.row_offset;
        let state = chunk.read_state();
        let start_ts = state
            .visibility
            .create_ts()
            .get(local)
            .copied()
            .unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            drop(state);
            // Chunk-routed base read: overlay first, then encoded base.
            return self.get_in(&chunks, row_idx);
        }
        let chain = state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(local))
            .cloned();
        drop(state);
        drop(chunks);
        let chain = chain?;
        if chain.is_empty() {
            return None;
        }
        // Version chain is ordered by start_ts ascending (oldest first).
        // Binary search finds the candidate interval containing query_ts
        // in O(log n) instead of O(n) linear scan.
        let idx = match chain.binary_search_by_key(&query_ts, |e| e.start_ts) {
            Ok(i) => i,
            Err(i) => {
                if i == 0 {
                    return None;
                }
                i - 1
            }
        };
        let entry = &chain[idx];
        if crate::mvcc_visibility::Visibility::is_version_visible(
            query_ts,
            entry.start_ts,
            entry.end_ts,
        ) {
            return entry.value.clone();
        }
        // After folding/GC intervals may have been merged; a single
        // predecessor check suffices for contiguous chains. Fall back
        // to neighbour check for the rare folded-gap case.
        if idx + 1 < chain.len() {
            let nxt = &chain[idx + 1];
            if crate::mvcc_visibility::Visibility::is_version_visible(
                query_ts,
                nxt.start_ts,
                nxt.end_ts,
            ) {
                return nxt.value.clone();
            }
        }
        None
    }

    /// Start timestamp of the version covering `query_ts` for a row.
    ///
    /// Internal companion of [`Column::get_at_ts`]: returns the stamp the
    /// value read was written at (the current `create_ts` when the current
    /// value covers `query_ts`, otherwise the covering before-image's
    /// `start_ts`, else 0 when no version covers it). Callers keep it as a
    /// record cache fence alongside the value.
    pub fn start_ts_at(&self, row_idx: usize, query_ts: Timestamp) -> Timestamp {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        let chunk = chunks.get(row_idx / capacity.max(1));
        let Some(chunk) = chunk else {
            return 0;
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return 0;
        }
        let local = row_idx - chunk.row_offset;
        let state = chunk.read_state();
        let start_ts = state
            .visibility
            .create_ts()
            .get(local)
            .copied()
            .unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            return start_ts;
        }
        let chain = state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(local))
            .cloned();
        drop(state);
        drop(chunks);
        chain
            .and_then(|chain| {
                if chain.is_empty() {
                    return None;
                }
                let idx = match chain.binary_search_by_key(&query_ts, |e| e.start_ts) {
                    Ok(i) => i,
                    Err(i) => {
                        if i == 0 {
                            return None;
                        }
                        i - 1
                    }
                };
                let entry = &chain[idx];
                if crate::mvcc_visibility::Visibility::is_version_visible(
                    query_ts,
                    entry.start_ts,
                    entry.end_ts,
                ) {
                    return Some(entry.start_ts);
                }
                if idx + 1 < chain.len() {
                    let nxt = &chain[idx + 1];
                    if crate::mvcc_visibility::Visibility::is_version_visible(
                        query_ts,
                        nxt.start_ts,
                        nxt.end_ts,
                    ) {
                        return Some(nxt.start_ts);
                    }
                }
                None
            })
            .unwrap_or(0)
    }

    /// Garbage-collect version-chain entries eligible under
    /// `Visibility::is_gc_eligible`, keeping one baseline entry when the
    /// retained chain would otherwise start after the cutoff.
    /// Exclusive-only (GC path): it rewrites every segment's chains.
    pub fn gc_versions(&self, min_active_snapshot_ts: Timestamp) -> usize {
        let chunks = self.chunks.read();
        let mut removed = 0;
        for chunk in chunks.iter() {
            let mut state = chunk.write_state();
            if let Some(chains) = state.version_chains.as_mut() {
                for chain in chains.iter_mut() {
                    let before = chain.len();
                    if chain.is_empty() {
                        continue;
                    }
                    let safe = min_active_snapshot_ts;
                    let mut after: Vec<VersionEntry> = Vec::new();
                    let mut last_before: Option<VersionEntry> = None;
                    for entry in chain.drain(..) {
                        if !crate::mvcc_visibility::Visibility::is_gc_eligible(entry.end_ts, safe) {
                            after.push(entry);
                        } else {
                            if last_before
                                .as_ref()
                                .is_none_or(|prev| entry.end_ts > prev.end_ts)
                            {
                                last_before = Some(entry);
                            }
                        }
                    }
                    let mut new_chain = after;
                    if let Some(lb) = last_before {
                        if new_chain.is_empty() {
                            // No interval covers safe, keep the most recent
                            // before-image as baseline if it is the only history.
                            // If the current value starts after safe, this entry
                            // is still the correct value for queries before that
                            // start but after safe. Keep it conservatively.
                            new_chain.push(lb);
                        } else {
                            let min_start = new_chain
                                .iter()
                                .map(|e| e.start_ts)
                                .min()
                                .unwrap_or(u64::MAX);
                            if min_start > safe {
                                new_chain.push(lb);
                                new_chain.sort_by_key(|e| e.start_ts);
                            }
                        }
                    }
                    removed += before - new_chain.len();
                    *chain = new_chain;
                }
            }
        }
        removed
    }

    /// Copy the MVCC row state (creation timestamp + before-image chain)
    /// from another column's row into this column's row.
    ///
    /// Unlike `set`, this preserves history: used when rows move between
    /// stores (e.g. table compaction rebuilds into fresh columns) so
    /// snapshot reads stay intact after the remap. Lazily allocates the
    /// destination chain only when the source actually retains history.
    /// Exclusive-only (compaction path): source and destination are both
    /// quiescent, but locking stays per-segment for uniformity.
    pub(crate) fn clone_row_state_from(&self, src: &Column, from: usize, to: usize) {
        let capacity = src.chunk_capacity();
        let src_chunks = src.chunks.read();
        let Some(src_chunk) = src_chunks.get(from / capacity.max(1)) else {
            return;
        };
        if from < src_chunk.row_offset || from >= src_chunk.row_offset + src_chunk.row_count {
            return;
        }
        let src_local = from - src_chunk.row_offset;
        let src_state = src_chunk.read_state();
        let src_create = src_state.visibility.create_ts().get(src_local).copied();
        let src_chain = src_state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(src_local))
            .cloned();
        let src_has_chains = src_state.version_chains.is_some();
        drop(src_state);
        drop(src_chunks);
        let dst_idx = self.ensure_coverage(to);
        let dst_chunks = self.chunks.read();
        let Some(dst_chunk) = dst_chunks.get(dst_idx) else {
            return;
        };
        if to < dst_chunk.row_offset || to >= dst_chunk.row_offset + dst_chunk.row_count {
            return;
        }
        let dst_local = to - dst_chunk.row_offset;
        let mut dst_state = dst_chunk.write_state();
        dst_state.visibility.ensure_len(dst_chunk.row_count);
        if let Some(create_ts) = src_create {
            if dst_local < dst_state.visibility.create_ts().len() {
                dst_state.visibility.mark_created(dst_local, create_ts);
            }
        }
        if src_has_chains {
            if dst_state.version_chains.is_none() {
                dst_state.version_chains = Some(vec![Vec::new(); dst_chunk.row_count]);
            }
            if let Some(vecs) = dst_state.version_chains.as_mut() {
                if vecs.len() < dst_chunk.row_count {
                    vecs.resize(dst_chunk.row_count, Vec::new());
                }
                if dst_local < vecs.len() {
                    vecs[dst_local] = src_chain.clone().unwrap_or_default();
                }
            }
        }
    }

    /// Every retained before-image value as global `(row, value)` pairs for
    /// exact zone rebuilds. Segment latches are taken one at a time and
    /// released before the caller widens zone state.
    pub(super) fn collect_chained_values(&self) -> Vec<(usize, Option<Value>)> {
        let chunks = self.chunks.read();
        let mut out = Vec::new();
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            if let Some(chains) = state.version_chains.as_ref() {
                for (local, chain) in chains.iter().enumerate() {
                    for entry in chain.iter() {
                        out.push((chunk.row_offset + local, entry.value.clone()));
                    }
                }
            }
        }
        out
    }

    #[cfg(test)]
    pub fn version_chain_len(&self, row_idx: usize) -> usize {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        chunks
            .get(row_idx / capacity.max(1))
            .filter(|chunk| {
                row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count
            })
            .map(|chunk| {
                chunk
                    .read_state()
                    .version_chains
                    .as_ref()
                    .and_then(|c| c.get(row_idx - chunk.row_offset))
                    .map(|c| c.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    pub fn version_chain_stats(&self) -> VersionChainStats {
        let chunks = self.chunks.read();
        let mut total_rows = 0usize;
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            if let Some(chains) = state.version_chains.as_ref() {
                total_rows += chains.len();
                for chain in chains.iter() {
                    total_entries += chain.len();
                    max_len = max_len.max(chain.len());
                    memory_bytes += chain.len() * std::mem::size_of::<VersionEntry>();
                    for entry in chain.iter() {
                        memory_bytes += entry
                            .value
                            .as_ref()
                            .map(super::value_payload_bytes)
                            .unwrap_or(0);
                    }
                }
            }
            memory_bytes += state.visibility.memory_usage();
        }
        let avg_len = if total_rows > 0 {
            total_entries as f64 / total_rows as f64
        } else {
            0.0
        };
        VersionChainStats {
            total_rows,
            total_entries,
            max_len,
            avg_len,
            memory_bytes,
        }
    }
}
