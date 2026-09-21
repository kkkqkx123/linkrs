use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::types::{CommitLsn, DataType, EdgeId, Timestamp};
use graphdb_core::Value;

pub(super) type EdgeTable = EdgeStore;

pub(super) fn watermark_at(ts: Timestamp) -> graphdb_transaction::MvccWatermarks {
    graphdb_transaction::MvccWatermarks::from_parts(ts, ts, None, CommitLsn::ZERO)
}

pub(super) fn create_test_schema() -> EdgeSchema {
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
        record_form: RecordForm::default(),
    }
}

/// Unified row space invariant: property mappings never outlive the
/// authority, and every live edge owns a property row.
pub(super) fn assert_row_space_unified(table: &EdgeTable) {
    for edge_id in table.properties.edge_ids() {
        assert!(
            table.mvcc.edge_timestamps.contains_key(&edge_id),
            "orphan property mapping for {:?}",
            edge_id
        );
    }
    for (edge_id, ts) in table.mvcc.edge_timestamps.iter() {
        if ts.delete_ts == Timestamp::MAX {
            assert!(
                table.properties.get_row_for_edge(edge_id).is_some(),
                "live edge {:?} without property row",
                edge_id
            );
        }
    }
    for (_, nbr) in table.out_csr.iter_all().chain(table.in_csr.iter_all()) {
        assert!(
            table.mvcc.edge_timestamps.contains_key(&nbr.edge_id),
            "orphan CSR row for {:?}",
            nbr.edge_id
        );
    }
}
