use graphdb_core::types::Timestamp;

use super::core::EdgeStore;
use crate::edge::node_group::ShardCsrIterator;
use crate::edge::EdgeRecord;

/// Streaming full-table edge scan.
///
/// Holds the underlying sharded row iterator and decodes one record per
/// `next()` call: no record vector is materialized at construction time, so
/// peak memory stays proportional to a single record. The `max_records`
/// limit is enforced on the advancing side inside `next()`. Groups are
/// visited in group order.
pub struct EdgeTableScanIterator<'a> {
    table: &'a EdgeStore,
    inner: ShardCsrIterator<'a>,
    ts: Timestamp,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
    projection: Option<Vec<String>>,
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
        Self {
            table,
            inner: table.out_csr.iter_all(),
            ts,
            max_records: None,
            current_count: 0,
            projection,
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
        // in group order, so no cross-group deduplication is needed.
        Self {
            table,
            inner: table.out_csr.iter_all(),
            ts,
            max_records,
            current_count: 0,
            projection: None,
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

        for (src_vid, nbr) in self.inner.by_ref() {
            if !self.table.mvcc.is_edge_visible(nbr.edge_id, self.ts) {
                continue;
            }
            self.current_count += 1;
            return Some(self.table.edge_record_from_nbr_projected(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                self.ts,
                self.projection.as_deref(),
            ));
        }
        None
    }
}
