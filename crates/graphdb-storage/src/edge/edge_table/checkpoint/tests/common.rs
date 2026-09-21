use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::Value;

pub(super) fn make_table() -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "weight".to_string(),
            data_type: graphdb_core::types::DataType::Double,
            nullable: false,
            default_value: Some(Value::Double(0.0)),
        }],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
}

pub(super) fn make_bounded_table(bound: usize) -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let config = EdgeTableConfig {
        max_append_ops_per_group: bound,
        ..EdgeTableConfig::default()
    };
    EdgeStore::with_config(schema, config).unwrap()
}
