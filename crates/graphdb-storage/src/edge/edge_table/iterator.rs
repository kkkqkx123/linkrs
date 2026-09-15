use graphdb_core::types::Timestamp;

use super::core::EdgeStore;
use crate::edge::EdgeRecord;

pub struct EdgeTableScanIterator<'a> {
    _table: &'a EdgeStore,
    records: std::vec::IntoIter<EdgeRecord>,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
}

impl<'a> EdgeTableScanIterator<'a> {
    pub fn new(table: &'a EdgeStore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    /// Create a scan iterator with a maximum record limit
    pub fn with_limit(table: &'a EdgeStore, ts: Timestamp, max_records: Option<usize>) -> Self {
        // Single-segment scan: every live entry in the CSR is visited once,
        // so no cross-segment deduplication is needed.
        let mut records = Vec::new();

        for (src_vid, nbr) in table.out_csr.iter(ts) {
            if !table.mvcc.is_edge_visible(nbr.edge_id, ts) {
                continue;
            }
            records.push(table.edge_record_from_nbr(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
            ));

            if let Some(max) = max_records {
                if records.len() >= max {
                    break;
                }
            }
        }

        Self {
            _table: table,
            records: records.into_iter(),
            max_records,
            current_count: 0,
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

        if let Some(record) = self.records.next() {
            self.current_count += 1;
            Some(record)
        } else {
            None
        }
    }
}
