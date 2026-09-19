//! Explicit freeze and unfreeze of edge-table groups per direction.
//!
//! Freezing packs one direction's group into the frozen form through the
//! shard-set freeze, promoting reclaimed deletions into the visibility
//! authority exactly like the group compaction path. Unfreezing rebuilds the
//! mutable variant with identical neighbor bytes and no authority traffic.

use super::core::EdgeStore;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::StorageResult;

impl EdgeStore {
    /// Freeze one group of one direction, returning its packed live count.
    ///
    /// `outgoing` selects the out or in shard set. Reclaimable tombstones
    /// below `cutoff` are dropped first and reported into the visibility
    /// authority; a maximum cutoff packs verbatim. The frozen group rejects
    /// writes until explicitly unfrozen.
    pub fn freeze_group(
        &mut self,
        outgoing: bool,
        gid: usize,
        cutoff: Timestamp,
        reserve_ratio: f32,
    ) -> StorageResult<u64> {
        if outgoing {
            let shards = &mut self.out_csr;
            let mvcc = &mut self.mvcc;
            shards.freeze_group(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id: EdgeId, delete_ts: Timestamp| {
                    mvcc.record_deletion(edge_id, delete_ts);
                },
            )
        } else {
            let shards = &mut self.in_csr;
            let mvcc = &mut self.mvcc;
            shards.freeze_group(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id: EdgeId, delete_ts: Timestamp| {
                    mvcc.record_deletion(edge_id, delete_ts);
                },
            )
        }
    }

    /// Unfreeze one group of one direction, returning its restored live count.
    pub fn unfreeze_group(&mut self, outgoing: bool, gid: usize) -> StorageResult<u64> {
        if outgoing {
            self.out_csr.unfreeze_group(gid)
        } else {
            self.in_csr.unfreeze_group(gid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::{DataType, Timestamp, VertexId};
    use graphdb_core::Value;

    use super::super::config::EdgeTableConfig;

    fn frozen_test_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        }
    }

    fn sample_table() -> EdgeStore {
        let mut table =
            EdgeStore::with_config(frozen_test_schema(), EdgeTableConfig::default()).unwrap();
        for src in 0..8u32 {
            for k in 0..2u32 {
                table
                    .insert_edge(
                        src,
                        src + k + 1,
                        0,
                        &[("weight".to_string(), Value::Double(1.0))],
                        100,
                    )
                    .unwrap();
            }
        }
        assert!(table.delete_edge(0, 1, 0, 200).unwrap());
        table
    }

    fn snapshot(table: &EdgeStore, ts: Timestamp) -> Vec<Vec<VertexId>> {
        (0..8u32)
            .map(|src| {
                table
                    .out_edges(src, ts)
                    .into_iter()
                    .map(|edge| edge.dst_vid)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn freeze_keeps_reads_and_rejects_writes() {
        let mut table = sample_table();
        let before_freeze = snapshot(&table, 300);
        let before_old = snapshot(&table, 150);

        let out_packed = table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
        let in_packed = table.freeze_group(false, 0, Timestamp::MAX, 0.0).unwrap();
        assert_eq!(out_packed, 15);
        assert_eq!(in_packed, 15);

        assert_eq!(snapshot(&table, 300), before_freeze);
        assert_eq!(snapshot(&table, 150), before_old);
        assert!(table.has_edge(0, 2, 0, 300));
        assert!(!table.has_edge(0, 1, 0, 300));
        assert!(table.has_edge(0, 1, 0, 150));

        assert!(table.insert_edge(1, 9, 0, &[], 400).is_err());
        assert!(table.delete_edge(1, 2, 0, 400).is_err());

        assert_eq!(table.unfreeze_group(true, 0).unwrap(), 15);
        assert_eq!(table.unfreeze_group(false, 0).unwrap(), 15);
        assert_eq!(snapshot(&table, 300), before_freeze);
        table.insert_edge(1, 9, 0, &[], 400).unwrap();
        assert!(table.has_edge(1, 9, 0, 400));
    }

    #[test]
    fn freeze_guards_bad_states() {
        let mut table = sample_table();
        assert!(table.freeze_group(true, 41, Timestamp::MAX, 0.0).is_err());
        assert!(table.unfreeze_group(true, 0).is_err());
        table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
        assert!(table.freeze_group(true, 0, Timestamp::MAX, 0.0).is_err());
    }
}
