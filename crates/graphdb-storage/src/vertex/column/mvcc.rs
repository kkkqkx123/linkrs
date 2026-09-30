use graphdb_core::types::Timestamp;
use graphdb_core::{StorageResult, Value};

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
    /// The liveness probe runs inside the same latch when the caller passes
    /// one through `set_versioned_checked`.
    pub fn set_versioned(
        &self,
        row_idx: usize,
        value: Option<&Value>,
        ts: Timestamp,
    ) -> graphdb_core::StorageResult<()> {
        self.set_versioned_impl(row_idx, value, ts, None::<fn() -> bool>)
    }

    /// Versioned write with row liveness rechecked inside the segment latch.
    ///
    /// The caller holds the identity read guard across the call and passes a
    /// probe reading the already-held timestamp map. A failed probe means a
    /// concurrent delete landed between validation and the segment write.
    pub fn set_versioned_checked<F: Fn() -> bool>(
        &self,
        row_idx: usize,
        value: Option<&Value>,
        ts: Timestamp,
        row_alive: F,
    ) -> graphdb_core::StorageResult<()> {
        self.set_versioned_impl(row_idx, value, ts, Some(row_alive))
    }

    fn set_versioned_impl<F: Fn() -> bool>(
        &self,
        row_idx: usize,
        value: Option<&Value>,
        ts: Timestamp,
        row_alive: Option<F>,
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
        let pre_covered = {
            let chunks = self.chunks.read();
            let capacity = self.chunk_capacity().max(1);
            chunks.get(row_idx / capacity).is_some_and(|chunk| {
                row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count
            })
        };
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            state.visibility.ensure_len(chunk.row_count);
            let live_create = state
                .visibility
                .create_ts()
                .get(local)
                .copied()
                .unwrap_or(0);
            if let Some(check) = row_alive.as_ref() {
                if !check() {
                    return Err(graphdb_core::StorageError::vertex_not_found());
                }
            }
            if let Some(chains) = state.version_chains.as_ref() {
                if let Some(chain) = chains.get(&local) {
                    if let Some(tail) = chain.last() {
                        if tail.start_ts >= ts || tail.end_ts > ts {
                            return Err(graphdb_core::StorageError::invalid_operation(format!(
                                "out-of-order version write on column {} row {}",
                                self.name, row_idx
                            )));
                        }
                    }
                }
            }
            if pre_covered && local < chunk.row_count && live_create < ts {
                let local_u32 = local as u32;
                let current: Option<Value> =
                    if super::overflow::OverflowStore::routes_for(&self.data_type) {
                        if let Some(handle) = state.overflow_rows.get(&local_u32).copied() {
                            let bytes =
                                self.overflow_store.lock().get(&handle).ok_or_else(|| {
                                    graphdb_core::StorageError::deserialize_error(format!(
                                        "column {} overflow payload missing at row {}",
                                        self.name, row_idx
                                    ))
                                })?;
                            Some(
                                super::overflow::decode_overflow_payload(&self.data_type, bytes)
                                    .map_err(|e| {
                                        graphdb_core::StorageError::deserialize_error(format!(
                                            "column {} overflow decode failed at row {}: {}",
                                            self.name, row_idx, e
                                        ))
                                    })?,
                            )
                        } else if let Some(hit) = state.overlay.get(local_u32) {
                            hit.clone()
                        } else if state.encoding.is_encoded() {
                            self.restore_string_type(state.encoding.get(local))
                        } else {
                            state.raw.as_storage().try_get(local).map_err(|e| {
                                graphdb_core::StorageError::deserialize_error(format!(
                                    "column {} raw decode failed at row {}: {}",
                                    self.name, row_idx, e
                                ))
                            })?
                        }
                    } else if let Some(hit) = state.overlay.get(local_u32) {
                        hit.clone()
                    } else if state.encoding.is_encoded() {
                        self.restore_string_type(state.encoding.get(local))
                    } else {
                        state.raw.as_storage().try_get(local).map_err(|e| {
                            graphdb_core::StorageError::deserialize_error(format!(
                                "column {} raw decode failed at row {}: {}",
                                self.name, row_idx, e
                            ))
                        })?
                    };
                let raw_null = state.raw.as_storage().is_null(local);
                if current.is_some() || raw_null {
                    if state.version_chains.is_none() {
                        state.version_chains = Some(std::collections::HashMap::new());
                    }
                    if let Some(chains) = state.version_chains.as_mut() {
                        chains.entry(local).or_default().push(VersionEntry {
                            start_ts: live_create,
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

    /// Search one row's version chain for the before-image covering
    /// `query_ts`. Chains stay ordered by `start_ts` ascending, so the
    /// candidate is located by binary search with a neighbour fallback for
    /// folded gaps. Returns the covering entry's stamp and value. Single
    /// shared implementation behind every versioned read entry.
    fn search_chain(
        chain: &[VersionEntry],
        query_ts: Timestamp,
    ) -> Option<(Timestamp, Option<Value>)> {
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
            return Some((entry.start_ts, entry.value.clone()));
        }
        if idx + 1 < chain.len() {
            let nxt = &chain[idx + 1];
            if crate::mvcc_visibility::Visibility::is_version_visible(
                query_ts,
                nxt.start_ts,
                nxt.end_ts,
            ) {
                return Some((nxt.start_ts, nxt.value.clone()));
            }
        }
        None
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
        // The chain is searched under the read guard and only the matched
        // value is cloned: cloning the whole chain per point read
        // allocates on every hot lookup of a long-chained row.
        let value = state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(&local))
            .and_then(|chain| Self::search_chain(chain, query_ts))
            .and_then(|(_, value)| value);
        drop(state);
        drop(chunks);
        value
    }

    /// Strict versioned read with explicit decode errors.
    ///
    /// Same visibility discipline as [`Self::get_at_ts`] through the unified
    /// predicate: column visibility only, row liveness stays with the caller.
    /// Never-written windows yield `Ok(None)`; corrupt base payloads yield
    /// `Err` with the column and row so queries fail loudly.
    pub fn try_get_at_ts(
        &self,
        row_idx: usize,
        query_ts: Timestamp,
    ) -> StorageResult<Option<Value>> {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        let Some(chunk) = chunks.get(row_idx / capacity.max(1)) else {
            return Ok(None);
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return Ok(None);
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
            return self.try_get_in(&chunks, row_idx);
        }
        let value = state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(&local))
            .and_then(|chain| Self::search_chain(chain, query_ts))
            .and_then(|(_, value)| value);
        drop(state);
        drop(chunks);
        Ok(value)
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
        let stamp = state
            .version_chains
            .as_ref()
            .and_then(|c| c.get(&local))
            .and_then(|chain| Self::search_chain(chain, query_ts))
            .map(|(stamp, _)| stamp)
            .unwrap_or(0);
        drop(state);
        drop(chunks);
        stamp
    }

    /// Backdate one row's creation stamp, keeping the smaller value.
    ///
    /// Offline redistribution only: freshly rebuilt rows carry the rebuild
    /// timestamp while their logical creation predates it. Backdating aligns
    /// the column layer with the migrated row stamp so snapshot reads below
    /// the rebuild timestamp still hit the current value instead of an
    /// empty version chain.
    pub fn backdate_created(&self, row_idx: usize, ts: Timestamp) {
        let _ = self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            if local < chunk.row_count {
                state.visibility.ensure_len(chunk.row_count);
                let current = state
                    .visibility
                    .create_ts()
                    .get(local)
                    .copied()
                    .unwrap_or(0);
                if current > ts {
                    state.visibility.mark_created(local, ts);
                }
            }
            Ok::<(), graphdb_core::StorageError>(())
        });
    }

    /// Combined value plus covering stamp read under one container hold.
    ///
    /// Merges the `get_at_ts` and `start_ts_at` passes so fenced point reads
    /// locate the chunk once instead of twice.
    pub fn get_with_stamp(
        &self,
        row_idx: usize,
        query_ts: Timestamp,
    ) -> (Timestamp, Option<Value>) {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        let Some(chunk) = chunks.get(row_idx / capacity.max(1)) else {
            return (0, None);
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return (0, None);
        }
        let local = row_idx - chunk.row_offset;
        let start_ts = chunk
            .read_state()
            .visibility
            .create_ts()
            .get(local)
            .copied()
            .unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            let value = self.get_in(&chunks, row_idx);
            return (start_ts, value);
        }
        let hit = chunk
            .read_state()
            .version_chains
            .as_ref()
            .and_then(|c| c.get(&local))
            .and_then(|chain| Self::search_chain(chain, query_ts));
        match hit {
            Some((stamp, value)) => (stamp, value),
            None => (0, None),
        }
    }

    /// Strict combined value plus covering stamp read under one container
    /// hold. Same merged pass as [`Self::get_with_stamp`] with explicit
    /// decode errors.
    pub fn try_get_with_stamp(
        &self,
        row_idx: usize,
        query_ts: Timestamp,
    ) -> graphdb_core::StorageResult<(Timestamp, Option<Value>)> {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        let Some(chunk) = chunks.get(row_idx / capacity.max(1)) else {
            return Ok((0, None));
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return Ok((0, None));
        }
        let local = row_idx - chunk.row_offset;
        let start_ts = chunk
            .read_state()
            .visibility
            .create_ts()
            .get(local)
            .copied()
            .unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            let value = self.try_get_in(&chunks, row_idx)?;
            return Ok((start_ts, value));
        }
        let hit = chunk
            .read_state()
            .version_chains
            .as_ref()
            .and_then(|c| c.get(&local))
            .and_then(|chain| Self::search_chain(chain, query_ts));
        match hit {
            Some((stamp, value)) => Ok((stamp, value)),
            None => Ok((0, None)),
        }
    }

    /// Undo a versioned write applied at `ts` on one row.
    ///
    /// Pops the before-image entry closed at `ts` and restores the previous
    /// value as current, so a failed commit leaves no versioned residue.
    /// Rows with no matching tail entry are a no-op. Runs under the segment
    /// latch like every other versioned mutation.
    pub fn undo_last_versioned_write(
        &self,
        row_idx: usize,
        ts: Timestamp,
    ) -> graphdb_core::StorageResult<()> {
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            let tail_matches = state
                .version_chains
                .as_ref()
                .and_then(|chains| chains.get(&local))
                .and_then(|chain| chain.last())
                .is_some_and(|tail| tail.end_ts == ts);
            if !tail_matches {
                return Ok(());
            }
            let Some(chains) = state.version_chains.as_mut() else {
                return Ok(());
            };
            let Some(chain) = chains.get_mut(&local) else {
                return Ok(());
            };
            let Some(entry) = chain.pop() else {
                return Ok(());
            };
            if chain.is_empty() {
                chains.remove(&local);
            }
            state.visibility.ensure_len(chunk.row_count);
            state.visibility.mark_created(local, entry.start_ts);
            let restore = entry.value.clone();
            self.write_core(chunk, state, row_idx, restore.as_ref(), use_chunk_layer)?;
            Ok(())
        })
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
                for chain in chains.values_mut() {
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
            .and_then(|c| c.get(&src_local))
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
                dst_state.version_chains = Some(std::collections::HashMap::new());
            }
            if let Some(vecs) = dst_state.version_chains.as_mut() {
                match src_chain.clone() {
                    Some(chain) if !chain.is_empty() => {
                        vecs.insert(dst_local, chain);
                    }
                    _ => {
                        vecs.remove(&dst_local);
                    }
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
                for (local, chain) in chains.iter() {
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
                    .and_then(|c| c.get(&(row_idx - chunk.row_offset)))
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
                for chain in chains.values() {
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
