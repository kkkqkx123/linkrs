use std::collections::HashSet;

use graphdb_core::types::Timestamp;

use super::core::TimeTravelEdgeStore;
use crate::edge::{EdgeRecord, Nbr};

pub struct EdgeTableScanIterator<'a> {
    _table: &'a TimeTravelEdgeStore,
    records: std::vec::IntoIter<EdgeRecord>,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
}

impl<'a> EdgeTableScanIterator<'a> {
    pub fn new(table: &'a TimeTravelEdgeStore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    /// Create a scan iterator with a maximum record limit
    pub fn with_limit(
        table: &'a TimeTravelEdgeStore,
        ts: Timestamp,
        max_records: Option<usize>,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for (src_vid, nbr) in table.out_csr.iter(ts) {
            if !table.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
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
        }

        if records.len() < max_records.unwrap_or(usize::MAX) {
            for segment in table.out_segments.iter().rev() {
                if segment.create_ts_min > ts {
                    continue;
                }

                for (src_vid, edge) in segment.csr.read().iter() {
                    if edge.timestamp <= ts
                        && !table.mvcc.is_tombstoned(edge.edge_id, ts)
                        && seen.insert(edge.edge_id)
                    {
                        records.push(table.edge_record_from_nbr(
                            src_vid.as_int64().unwrap_or(0) as u32,
                            Nbr::with_prop_offset(
                                edge.endpoint,
                                edge.rank,
                                edge.edge_id,
                                edge.prop_offset,
                            ),
                            ts,
                        ));

                        if let Some(max) = max_records {
                            if records.len() >= max {
                                break;
                            }
                        }
                    }
                }

                if let Some(max) = max_records {
                    if records.len() >= max {
                        break;
                    }
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
