use graphdb_core::types::{Timestamp, TransactionId};
use graphdb_core::Value;

use super::Column;

/// One before-image of a row's value, valid on `[start_ts, end_ts)`.
///
/// The version chain per row stores the newest entry first. The current value
/// lives in the column storage and is valid from `visibility.create_ts` onward;
/// each write pushes the previous current value here with its lifetime.
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
    commit_ts: Vec<Timestamp>,
    pending_owner: Vec<Option<TransactionId>>,
    len: usize,
}

impl RowVisibility {
    pub fn new() -> Self {
        Self {
            create_ts: Vec::new(),
            commit_ts: Vec::new(),
            pending_owner: Vec::new(),
            len: 0,
        }
    }

    #[inline]
    pub fn mark_created(&mut self, row_idx: usize, ts: Timestamp) {
        self.ensure_len(row_idx + 1);
        self.create_ts[row_idx] = ts;
        self.commit_ts[row_idx] = ts;
        self.pending_owner[row_idx] = None;
        if row_idx + 1 > self.len {
            self.len = row_idx + 1;
        }
    }

    /// Mark a row as pending (write timestamp allocated but not yet committed).
    /// The row is visible only to `owner`; other transactions see the previous
    /// committed value until `mark_committed` publishes the commit timestamp.
    pub fn mark_created_pending(
        &mut self,
        row_idx: usize,
        write_ts: Timestamp,
        owner: TransactionId,
    ) {
        self.ensure_len(row_idx + 1);
        self.create_ts[row_idx] = write_ts;
        self.commit_ts[row_idx] = Timestamp::MAX;
        self.pending_owner[row_idx] = Some(owner);
        if row_idx + 1 > self.len {
            self.len = row_idx + 1;
        }
    }

    /// Publish a pending write as committed at `commit_ts`. `write_ts` must
    /// match the pending write's create timestamp to guard against stale publishes.
    pub fn mark_committed(&mut self, row_idx: usize, commit_ts: Timestamp) -> bool {
        if row_idx >= self.len {
            return false;
        }
        if self.commit_ts[row_idx] != Timestamp::MAX {
            // Already committed; update to latest commit timestamp (e.g. retry).
            self.commit_ts[row_idx] = commit_ts;
            return true;
        }
        self.commit_ts[row_idx] = commit_ts;
        self.pending_owner[row_idx] = None;
        true
    }

    pub fn create_ts(&self) -> &[Timestamp] {
        &self.create_ts
    }

    pub fn commit_ts(&self) -> &[Timestamp] {
        &self.commit_ts
    }

    pub fn pending_owner(&self, row_idx: usize) -> Option<TransactionId> {
        self.pending_owner.get(row_idx).copied().flatten()
    }

    pub fn is_pending(&self, row_idx: usize) -> bool {
        self.commit_ts
            .get(row_idx)
            .is_some_and(|ts| *ts == Timestamp::MAX)
    }

    pub fn ensure_len(&mut self, n: usize) {
        if self.create_ts.len() < n {
            self.create_ts.resize(n, 0);
            self.commit_ts.resize(n, 0);
            self.pending_owner.resize(n, None);
        }
        if self.len < n {
            self.len = n;
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        self.create_ts.reserve(additional);
        self.commit_ts.reserve(additional);
        self.pending_owner.reserve(additional);
    }

    pub fn resize(&mut self, new_len: usize) {
        self.create_ts.resize(new_len, 0);
        self.commit_ts.resize(new_len, 0);
        self.pending_owner.resize(new_len, None);
        self.len = new_len;
    }

    pub fn clear(&mut self) {
        self.create_ts.clear();
        self.commit_ts.clear();
        self.pending_owner.clear();
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn memory_usage(&self) -> usize {
        self.create_ts.len() * std::mem::size_of::<Timestamp>()
            + self.commit_ts.len() * std::mem::size_of::<Timestamp>()
            + self.pending_owner.len() * std::mem::size_of::<Option<TransactionId>>()
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
            if let Some(chains) = self.version_chains.as_mut() {
                chains.resize(n, Vec::new());
            }
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
                if self.version_chains.is_none() {
                    self.version_chains = Some(vec![Vec::new(); self.visibility.len()]);
                }
                if let Some(chains) = self.version_chains.as_mut() {
                    if row_idx >= chains.len() {
                        chains.resize(row_idx + 1, Vec::new());
                    }
                    chains[row_idx].push(VersionEntry {
                        start_ts: old_create,
                        end_ts: ts,
                        value: current,
                    });
                }
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
    pub fn get_at_ts(&self, row_idx: usize, query_ts: Timestamp) -> Option<Value> {
        let start_ts = self.visibility.commit_ts.get(row_idx).copied().unwrap_or(0);
        if crate::mvcc_visibility::Visibility::is_column_visible(query_ts, start_ts) {
            return if self.encoding.is_encoded() {
                self.encoding.get(row_idx)
            } else {
                self.inner().get(row_idx)
            };
        }
        if let Some(chains) = self.version_chains.as_ref() {
            if let Some(chain) = chains.get(row_idx) {
                if !chain.is_empty() {
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
                }
            }
        }
        None
    }

    /// Garbage-collect using unified watermarks. `watermarks` is the single
    /// GC frontier captured at pass start so all table types share the same
    /// cutoff. `margin` is subtracted conservatively inside this helper.
    #[allow(dead_code)]
    pub fn gc_versions_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let safe = watermarks.safe_gc_timestamp_with_margin(margin);
        self.gc_versions(safe)
    }

    /// Garbage-collect version-chain entries no longer visible to any active
    /// snapshot at `min_active_snapshot_ts`. Returns the number of entries
    /// removed. Keeps the latest entry that ends at or before the cutoff when
    /// it is needed as a baseline for queries that fall into a gap before the
    /// next retained interval.
    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        let mut removed = 0;
        if let Some(chains) = self.version_chains.as_mut() {
            for chain in chains.iter_mut() {
                let before = chain.len();
                if chain.is_empty() {
                    continue;
                }
                let safe = min_active_snapshot_ts;
                let mut after: Vec<VersionEntry> = Vec::new();
                let mut last_before: Option<VersionEntry> = None;
                for entry in chain.drain(..) {
                    if entry.end_ts > safe {
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
        removed
    }

    /// Snapshot the MVCC metadata of `from` into `to` (used by table
    /// compaction to preserve version history when rows are remapped).
    pub(crate) fn copy_row_state(&mut self, from: usize, to: usize) {
        if from >= self.len() {
            return;
        }
        self.ensure_row_meta(to + 1);
        if let Some(chains) = self.version_chains.as_mut() {
            if from < chains.len() && to < chains.len() {
                chains[to] = chains[from].clone();
            }
        }
        if from < self.visibility.create_ts.len() && to < self.visibility.create_ts.len() {
            let create = self.visibility.create_ts[from];
            let commit = self.visibility.commit_ts[from];
            let owner = self.visibility.pending_owner[from];
            self.visibility.create_ts[to] = create;
            self.visibility.commit_ts[to] = commit;
            self.visibility.pending_owner[to] = owner;
        }
    }

    pub fn version_chain_len(&self, row_idx: usize) -> usize {
        self.version_chains
            .as_ref()
            .and_then(|chains| chains.get(row_idx))
            .map(|c| c.len())
            .unwrap_or(0)
    }

    pub fn version_chain_stats(&self) -> VersionChainStats {
        let total_rows = self.version_chains.as_ref().map(|v| v.len()).unwrap_or(0);
        let total_entries: usize = self
            .version_chains
            .as_ref()
            .map(|chains| chains.iter().map(|c| c.len()).sum())
            .unwrap_or(0);
        let max_len = self
            .version_chains
            .as_ref()
            .map(|chains| chains.iter().map(|c| c.len()).max().unwrap_or(0))
            .unwrap_or(0);
        let avg_len = if total_rows > 0 {
            total_entries as f64 / total_rows as f64
        } else {
            0.0
        };
        let memory_bytes = self
            .version_chains
            .as_ref()
            .map(|chains| {
                chains
                    .iter()
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
    }

    pub fn fold_oldest(&mut self, row_idx: usize, cap: usize, horizon: Timestamp) {
        if cap == 0 {
            return;
        }
        let Some(chains) = self.version_chains.as_mut() else {
            return;
        };
        let Some(chain) = chains.get_mut(row_idx) else {
            return;
        };
        // Fold from the front: merge the second-newest entry into the newest,
        // preserving the most recent value while extending its visible time
        // range. This maintains the expected interval-merge semantics where
        // recent history stays exact and oldest intervals are folded.
        while chain.len() > cap {
            if chain.len() < 2 {
                break;
            }
            let can_fold_horizon = if horizon == Timestamp::MAX {
                true
            } else {
                chain[1].end_ts <= horizon
            };
            if !can_fold_horizon {
                break;
            }
            let second = chain.remove(1);
            if chain[0].end_ts < second.end_ts {
                chain[0].end_ts = second.end_ts;
            }
            let _ = second;
        }
    }

    pub fn clear_row_version_chains(&mut self, row_idx: usize) {
        if let Some(chains) = self.version_chains.as_mut() {
            if row_idx < chains.len() {
                chains[row_idx].clear();
            }
        }
        if row_idx < self.visibility.create_ts.len() {
            self.visibility.create_ts[row_idx] = 0;
            self.visibility.commit_ts[row_idx] = 0;
            self.visibility.pending_owner[row_idx] = None;
        }
    }

    /// Optional accessor for lazy-allocated chains.
    pub fn version_chains_opt(&self) -> Option<&Vec<Vec<VersionEntry>>> {
        self.version_chains.as_ref()
    }

    /// Directly set the optional version chains (used by V2 serialization).
    #[allow(unused)]
    pub fn set_version_chains_opt(&mut self, v: Option<Vec<Vec<VersionEntry>>>) {
        self.version_chains = v;
        if let Some(chains) = self.version_chains.as_ref() {
            self.visibility.ensure_len(chains.len());
        }
    }
}
