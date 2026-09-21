use std::collections::HashSet;

use graphdb_core::types::Timestamp;

use super::core::EdgeStore;
use super::stats::ScanPruneReport;
use crate::cursor::ScanPredicate;
use crate::edge::node_group::ShardCsrIterator;
use crate::edge::{EdgeRecord, Nbr};

/// Default adjacency batch size for batched traversal.
pub const DEFAULT_ADJACENCY_BATCH: usize = 256;

/// Reusable-buffer batch accessor for adjacency reads.
///
/// Traversal and batch queries use the accessor with a caller-provided
/// buffer so peak memory stays proportional to the batch size instead of
/// the row degree. The buffer is valid only for the snapshot timestamp the
/// accessor was created with and must not be held across snapshots.
/// Visibility still goes through the version authority; no second decision
/// source exists.
pub struct AdjacencyBatchAccessor<'a> {
    table: &'a EdgeStore,
    outgoing: bool,
    ts: Timestamp,
}

impl<'a> AdjacencyBatchAccessor<'a> {
    pub fn new(table: &'a EdgeStore, outgoing: bool, ts: Timestamp) -> Self {
        Self {
            table,
            outgoing,
            ts,
        }
    }

    fn csr(&self) -> &crate::edge::node_group::CsrShardSet {
        if self.outgoing {
            &self.table.out_csr
        } else {
            &self.table.in_csr
        }
    }

    /// Fill a caller buffer with every visible neighbor of one row.
    ///
    /// Clears the buffer first and never allocates internally beyond the
    /// buffer growth the caller owns. Missing legs fill nothing.
    pub fn fill_into(&self, src: u32, out: &mut Vec<Nbr>) {
        out.clear();
        if !self.table.is_direction_available(self.outgoing) {
            return;
        }
        self.table.fill_visible_into(self.csr(), src, self.ts, out);
    }

    /// Fill a caller buffer with the first `limit` visible neighbors.
    ///
    /// Stops the physical visit after `limit` hits so high-degree `LIMIT`
    /// queries never decode the full adjacency. Missing legs fill nothing.
    pub fn fill_limited(&self, src: u32, out: &mut Vec<Nbr>, limit: usize) {
        out.clear();
        if limit == 0 || !self.table.is_direction_available(self.outgoing) {
            return;
        }
        out.reserve(limit.min(32));
        let table = self.table;
        let ts = self.ts;
        self.csr().visit_physical(src, |nbr| {
            if table.is_visible(nbr.edge_id, ts) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
    }

    /// Visit visible neighbors in fixed-size batches reusing one caller
    /// buffer. The callback receives each full batch slice and returns false
    /// to stop early. Peak memory stays proportional to `batch_size`. A zero
    /// `batch_size` selects the default adjacency batch.
    pub fn visit_batched<F>(&self, src: u32, scratch: &mut Vec<Nbr>, batch_size: usize, mut f: F)
    where
        F: FnMut(&[Nbr]) -> bool,
    {
        let batch_size = if batch_size == 0 {
            DEFAULT_ADJACENCY_BATCH
        } else {
            batch_size
        };
        scratch.clear();
        if !self.table.is_direction_available(self.outgoing) {
            return;
        }
        let table = self.table;
        let ts = self.ts;
        let mut done = false;
        self.csr().visit_physical(src, |nbr| {
            if done {
                return false;
            }
            if table.is_visible(nbr.edge_id, ts) {
                scratch.push(nbr);
                if scratch.len() >= batch_size {
                    if !f(scratch.as_slice()) {
                        done = true;
                        return false;
                    }
                    scratch.clear();
                }
            }
            true
        });
        if !done && !scratch.is_empty() {
            f(scratch.as_slice());
            scratch.clear();
        }
    }

    /// Point lookup through the shared merged row-location logic.
    pub fn lookup(&self, src: u32, dst: u32, rank: i64) -> Option<Nbr> {
        if !self.table.is_direction_available(self.outgoing) {
            return None;
        }
        let csr = self.csr();
        let ts = self.ts;
        let table = self.table;
        let mut found = None;
        csr.visit_physical(src, |nbr| {
            if nbr.endpoint == dst && nbr.rank == rank && table.is_visible(nbr.edge_id, ts) {
                found = Some(nbr);
                false
            } else {
                true
            }
        });
        found
    }
}

/// Streaming full-table edge scan.
///
/// Holds the underlying sharded row iterator and decodes one record per
/// `next()` call: no record vector is materialized at construction time, so
/// peak memory stays proportional to a single record. The `max_records`
/// limit is enforced on the advancing side inside `next()`. Groups are
/// visited in group order.
///
/// Segment pruning runs before decoding: groups whose flushed statistics
/// provably exclude the pushed predicates are skipped without touching
/// property columns, and per-row predicate checks at the column-scan layer
/// drop misses before the full projection decodes. Pruning is a pure
/// pre-filter; the prune report exposes the observed prune and filter rates.
pub struct EdgeTableScanIterator<'a> {
    table: &'a EdgeStore,
    inner: ShardCsrIterator<'a>,
    outgoing: bool,
    ts: Timestamp,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
    projection: Option<Vec<String>>,
    predicates: Vec<ScanPredicate>,
    pruned_groups: HashSet<usize>,
    segments_total: usize,
    segments_pruned: usize,
    rows_scanned: usize,
    rows_filtered: usize,
    /// Row id of the last visited entry plus its prune verdict. Physical
    /// entries arrive row by row, so the group routing plus prune lookup
    /// runs once per row instead of once per entry.
    last_row: Option<u32>,
    last_row_pruned: bool,
}

impl<'a> EdgeTableScanIterator<'a> {
    pub fn new(table: &'a EdgeStore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    pub fn with_projection(
        table: &'a EdgeStore,
        ts: Timestamp,
        projection: Option<Vec<String>>,
    ) -> Self {
        Self::with_predicates(table, ts, projection, Vec::new())
    }

    /// Scan with pushed equality and range predicates.
    ///
    /// Predicates filter at the column-scan layer before the full projection
    /// decodes; segment statistics prune whole groups first. Uses the same
    /// row-location logic as adjacency and point lookups, and never builds
    /// an intermediate materialized iterator. Scans the stored leg so
    /// single-direction tables iterate their one leg instead of an empty one.
    pub fn with_predicates(
        table: &'a EdgeStore,
        ts: Timestamp,
        projection: Option<Vec<String>>,
        predicates: Vec<ScanPredicate>,
    ) -> Self {
        let outgoing = table.has_out_edges();
        let shards = if outgoing {
            &table.out_csr
        } else {
            &table.in_csr
        };
        let existing = shards.existing_group_ids();
        let segments_total = existing.len();
        let mut pruned_groups = HashSet::new();
        if !predicates.is_empty() {
            for gid in &existing {
                if !table.segment_may_contain(*gid as u32, &predicates) {
                    pruned_groups.insert(*gid);
                }
            }
        }
        let segments_pruned = pruned_groups.len();
        Self {
            table,
            inner: shards.iter_all(),
            outgoing,
            ts,
            max_records: None,
            current_count: 0,
            projection,
            predicates,
            pruned_groups,
            segments_total,
            segments_pruned,
            rows_scanned: 0,
            rows_filtered: 0,
            last_row: None,
            last_row_pruned: false,
        }
    }

    /// Observed prune and filter rates for this scan.
    pub fn prune_report(&self) -> ScanPruneReport {
        ScanPruneReport {
            segments_total: self.segments_total,
            segments_pruned: self.segments_pruned,
            rows_scanned: self.rows_scanned,
            rows_filtered: self.rows_filtered,
        }
    }

    /// Create a scan iterator with a maximum record limit.
    ///
    /// The limit is enforced while advancing, not at construction:
    /// constructing this iterator performs no table access. Physical entries
    /// are visited and visibility is decided by the version authority alone;
    /// row stamps never filter reads.
    pub fn with_limit(table: &'a EdgeStore, ts: Timestamp, max_records: Option<usize>) -> Self {
        // Sharded scan: every physical entry in every group is visited once
        // in group order, so no cross-group deduplication is needed. The
        // stored leg is scanned so single-direction tables stay scannable.
        let outgoing = table.has_out_edges();
        let shards = if outgoing {
            &table.out_csr
        } else {
            &table.in_csr
        };
        let existing = shards.existing_group_ids();
        let segments_total = existing.len();
        Self {
            table,
            inner: shards.iter_all(),
            outgoing,
            ts,
            max_records,
            current_count: 0,
            projection: None,
            predicates: Vec::new(),
            pruned_groups: HashSet::new(),
            segments_total,
            segments_pruned: 0,
            rows_scanned: 0,
            rows_filtered: 0,
            last_row: None,
            last_row_pruned: false,
        }
    }
}

impl<'a> Iterator for EdgeTableScanIterator<'a> {
    type Item = EdgeRecord;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(max) = self.max_records {
            if self.current_count >= max {
                return None;
            }
        }

        for (row_vid, nbr) in self.inner.by_ref() {
            self.rows_scanned += 1;
            let row = row_vid.as_int64().unwrap_or(0) as u32;
            // Row-granular prune verdict: entries arrive row by row, so a
            // pruned row skips every remaining entry without another group
            // lookup, and the limit below stops the walk as soon as enough
            // records are collected.
            let pruned = match self.last_row {
                Some(cached) if cached == row => self.last_row_pruned,
                _ => {
                    let shards = if self.outgoing {
                        &self.table.out_csr
                    } else {
                        &self.table.in_csr
                    };
                    let pruned = shards
                        .group_of(row)
                        .is_some_and(|gid| self.pruned_groups.contains(&gid));
                    self.last_row = Some(row);
                    self.last_row_pruned = pruned;
                    pruned
                }
            };
            if pruned {
                continue;
            }
            // Single authority verdict per entry: predicate and projection
            // below reuse it instead of querying the authority twice more.
            if !self.table.is_visible(nbr.edge_id, self.ts) {
                continue;
            }
            if !self.predicates.is_empty()
                && !self.table.matches_pushdown_assume_visible(
                    nbr.edge_id,
                    self.ts,
                    &self.predicates,
                )
            {
                self.rows_filtered += 1;
                continue;
            }
            self.current_count += 1;
            if self.outgoing {
                return Some(self.table.edge_record_from_nbr_projected_assume_visible(
                    row,
                    nbr,
                    self.ts,
                    self.projection.as_deref(),
                ));
            }
            return Some(self.table.edge_record_from_in_nbr_assume_visible(
                row,
                nbr,
                self.ts,
                self.projection.as_deref(),
            ));
        }
        None
    }
}
