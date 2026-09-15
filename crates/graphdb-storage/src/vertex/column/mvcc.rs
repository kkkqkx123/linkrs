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
    len: usize,
}

impl RowVisibility {
    pub fn new() -> Self {
        Self {
            create_ts: Vec::new(),
            len: 0,
        }
    }

    #[inline]
    pub fn mark_created(&mut self, row_idx: usize, ts: Timestamp) {
        self.ensure_len(row_idx + 1);
        self.create_ts[row_idx] = ts;
        if row_idx + 1 > self.len {
            self.len = row_idx + 1;
        }
    }

    pub fn create_ts(&self) -> &[Timestamp] {
        &self.create_ts
    }

    pub fn ensure_len(&mut self, n: usize) {
        if self.create_ts.len() < n {
            self.create_ts.resize(n, 0);
        }
        if self.len < n {
            self.len = n;
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        self.create_ts.reserve(additional);
    }

    pub fn resize(&mut self, new_len: usize) {
        self.create_ts.resize(new_len, 0);
        self.len = new_len;
    }

    pub fn clear(&mut self) {
        self.create_ts.clear();
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
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
    /// Ensure the MVCC metadata vectors are at least `n` rows long.
    /// New (never-written) rows default to start_ts 0, i.e. their loaded or
    /// not-yet-written value is treated as current.
    pub(super) fn ensure_row_meta(&mut self, n: usize) {
        if self.visibility.len() < n {
            self.with_version_chains_write(|chains| {
                if let Some(chains) = chains.as_mut() {
                    chains.resize(n, Vec::new());
                }
            });
        }
        self.visibility.ensure_len(n);
    }

    /// Versioned write: records the current value as a before-image valid on
    /// `[create_ts, ts)`, then stores `value` as the current value valid
    /// from `ts` onward. Rows written for the first time get no before-image.
    pub fn set_versioned(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
        ts: Timestamp,
    ) -> graphdb_core::StorageResult<()> {
        self.ensure_row_meta(row_idx + 1);
        let old_create = self
            .visibility
            .create_ts()
            .get(row_idx)
            .copied()
            .unwrap_or(0);
        // Only record a before-image when the current value genuinely predates
        // this write (guards against zero-length ranges from rollback writes
        // that reuse the transaction's original timestamp).
        if row_idx < self.len() && old_create < ts {
            let current = self.get(row_idx);
            if current.is_some() || self.is_null(row_idx) {
                // Capture visibility length before closure to avoid borrow conflict.
                let vis_len = self.visibility.len();
                // Use with_version_chains_write for controlled mutable access.
                self.with_version_chains_write(|chains| {
                    if chains.is_none() {
                        *chains = Some(vec![Vec::new(); vis_len]);
                    }
                    if let Some(chains) = chains.as_mut() {
                        if row_idx >= chains.len() {
                            chains.resize(row_idx + 1, Vec::new());
                        }
                        chains[row_idx].push(VersionEntry {
                            start_ts: old_create,
                            end_ts: ts,
                            value: current,
                        });
                    }
                });
            }
        }
        self.write_value(row_idx, value)?;
        self.visibility.mark_created(row_idx, ts);
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
        let start_ts = self.visibility.create_ts.get(row_idx).copied().unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            // Chunk-routed base read: overlay first, then encoded base.
            return self.get(row_idx);
        }
        self.with_version_chains_read(|chains| {
            chains.and_then(|c| c.get(row_idx)).and_then(|chain| {
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
            })
        })
    }

    /// Start timestamp of the version covering `query_ts` for a row.
    ///
    /// Internal companion of [`Column::get_at_ts`]: returns the stamp the
    /// value read was written at (the current `create_ts` when the current
    /// value covers `query_ts`, otherwise the covering before-image's
    /// `start_ts`, else 0 when no version covers it). Pending-aware point
    /// lookups use it to detect a value written by a foreign uncommitted
    /// transaction and fall back to `stamp - 1`.
    pub fn start_ts_at(&self, row_idx: usize, query_ts: Timestamp) -> Timestamp {
        let start_ts = self.visibility.create_ts.get(row_idx).copied().unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            return start_ts;
        }
        self.with_version_chains_read(|chains| {
            chains
                .and_then(|c| c.get(row_idx))
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
        })
    }

    /// Garbage-collect version-chain entries eligible under
    /// `Visibility::is_gc_eligible`, keeping one baseline entry when the
    /// retained chain would otherwise start after the cutoff.
    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        let mut removed = 0;
        self.with_version_chains_write(|chains| {
            if let Some(chains) = chains.as_mut() {
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
        });
        removed
    }

    /// Copy the MVCC row state (creation timestamp + before-image chain)
    /// from another column's row into this column's row.
    ///
    /// Unlike `set`, this preserves history: used when rows move between
    /// stores (e.g. table compaction rebuilds into fresh columns) so
    /// snapshot reads stay intact after the remap. Lazily allocates the
    /// destination chain only when the source actually retains history.
    pub(crate) fn clone_row_state_from(&mut self, src: &Column, from: usize, to: usize) {
        let src_create = src.visibility.create_ts().get(from).copied();
        let src_has_chains = src.with_version_chains_read(|chains| chains.is_some());
        let src_chain =
            src.with_version_chains_read(|chains| chains.and_then(|c| c.get(from)).cloned());
        self.ensure_row_meta(to + 1);
        if let Some(create_ts) = src_create {
            if to < self.visibility.create_ts.len() {
                self.visibility.create_ts[to] = create_ts;
            }
        }
        if src_has_chains {
            self.with_version_chains_write(|dst| {
                if dst.is_none() {
                    *dst = Some(vec![Vec::new(); to + 1]);
                }
                if let Some(vecs) = dst.as_mut() {
                    if to >= vecs.len() {
                        vecs.resize(to + 1, Vec::new());
                    }
                    vecs[to] = src_chain.clone().unwrap_or_default();
                }
            });
        }
    }

    #[cfg(test)]
    pub fn version_chain_len(&self, row_idx: usize) -> usize {
        self.with_version_chains_read(|chains| {
            chains
                .and_then(|c| c.get(row_idx))
                .map(|c| c.len())
                .unwrap_or(0)
        })
    }

    pub fn version_chain_stats(&self) -> VersionChainStats {
        self.with_version_chains_read(|chains| {
            let total_rows = chains.map(|v| v.len()).unwrap_or(0);
            let total_entries: usize = chains
                .map(|c| c.iter().map(|chain| chain.len()).sum())
                .unwrap_or(0);
            let max_len = chains
                .map(|c| c.iter().map(|chain| chain.len()).max().unwrap_or(0))
                .unwrap_or(0);
            let avg_len = if total_rows > 0 {
                total_entries as f64 / total_rows as f64
            } else {
                0.0
            };
            let memory_bytes = chains
                .map(|c| {
                    c.iter()
                        .map(|chain| {
                            chain.len() * std::mem::size_of::<VersionEntry>()
                                + chain
                                    .iter()
                                    .map(|e| {
                                        e.value
                                            .as_ref()
                                            .map(super::value_payload_bytes)
                                            .unwrap_or(0)
                                    })
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0)
                + self.visibility.memory_usage();
            VersionChainStats {
                total_rows,
                total_entries,
                max_len,
                avg_len,
                memory_bytes,
            }
        })
    }

    /// Execute a closure with read-only access to the version chains.
    ///
    /// This method enables concurrent version chain reads by providing
    /// controlled access to the internal `Option<Vec<Vec<VersionEntry>>>`.
    /// Multiple threads can call this method simultaneously since it only
    /// requires `&self`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stats = column.with_version_chains_read(|chains| {
    ///     chains.map(|c| c.iter().map(|chain| chain.len()).sum::<usize>())
    ///          .unwrap_or(0)
    /// });
    /// ```
    pub fn with_version_chains_read<R>(
        &self,
        f: impl FnOnce(Option<&Vec<Vec<VersionEntry>>>) -> R,
    ) -> R {
        f(self.version_chains.as_ref())
    }

    /// Execute a closure with exclusive access to the version chains.
    ///
    /// This method provides controlled mutable access to the internal
    /// `Option<Vec<Vec<VersionEntry>>>`. Only one thread can call this
    /// method at a time since it requires `&mut self`.
    pub fn with_version_chains_write<R>(
        &mut self,
        f: impl FnOnce(&mut Option<Vec<Vec<VersionEntry>>>) -> R,
    ) -> R {
        f(&mut self.version_chains)
    }
}
