//! Integration tests: non-scalar property values survive the undo path.
//!
//! Regression tests for the silent `PropertyValue` -> `Value` lossy conversion:
//! undo log entries must preserve the exact old value of non-scalar properties
//! (Decimal128, Date, DateTime, List, Map, Vector, ...) through postcard
//! serialization and rollback, restoring the original value instead of Null.

use graphdb_core::core::types::storage_ids::{EdgeIdentifier, VertexIdentifier};
use graphdb_core::core::value::date_time::{DateTimeValue, DateValue};
use graphdb_core::core::value::decimal128::Decimal128Value;
use graphdb_core::core::value::null::NullType;
use graphdb_core::core::value::uuid::UuidValue;
use graphdb_core::core::value::List;
use graphdb_core::core::Value;
use graphdb_transaction::transaction::undo_log::{
    UndoLogEntry, UndoLogError, UndoLogResult, UndoTarget, UpdateEdgePropUndo, UpdateVertexPropUndo,
};
use graphdb_transaction::transaction::wal::{ColumnId, LabelId, Timestamp, VertexId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct RecordingUndoTarget {
    restored: Mutex<Vec<Value>>,
}

impl RecordingUndoTarget {
    fn new() -> Self {
        Self {
            restored: Mutex::new(Vec::new()),
        }
    }

    fn restored(&self) -> Vec<Value> {
        self.restored.lock().expect("poisoned").clone()
    }
}

impl UndoTarget for RecordingUndoTarget {
    fn delete_vertex_type(&self, _label: LabelId) -> UndoLogResult<()> {
        Ok(())
    }

    fn delete_edge_type(&self, _edge_key: graphdb_core::core::types::EdgeKey) -> UndoLogResult<()> {
        Ok(())
    }

    fn delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
        Ok(())
    }

    fn delete_edge(
        &self,
        _edge_ctx: graphdb_core::core::types::EdgeDeletionContext,
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn undo_update_vertex_property(
        &self,
        _vertex: VertexIdentifier,
        _col_id: ColumnId,
        value: Value,
        _ts: Timestamp,
    ) -> UndoLogResult<()> {
        self.restored.lock().expect("poisoned").push(value);
        Ok(())
    }

    fn undo_update_edge_property(
        &self,
        _edge_id: EdgeIdentifier,
        _col_id: ColumnId,
        value: Value,
        _ts: Timestamp,
    ) -> UndoLogResult<()> {
        self.restored.lock().expect("poisoned").push(value);
        Ok(())
    }

    fn revert_delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_delete_edge(
        &self,
        _edge_ctx: graphdb_core::core::types::EdgeDeletionContext,
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_delete_vertex_properties(
        &self,
        _label_name: &str,
        _prop_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_delete_edge_properties(
        &self,
        _src_label: &str,
        _dst_label: &str,
        _edge_label: &str,
        _prop_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_delete_edge_label(
        &self,
        _src_label: &str,
        _dst_label: &str,
        _edge_label: &str,
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_rename_vertex_properties(
        &self,
        _label_name: &str,
        _current_names: &[String],
        _original_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }

    fn revert_rename_edge_properties(
        &self,
        _src_label: &str,
        _dst_label: &str,
        _edge_label: &str,
        _current_names: &[String],
        _original_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }
}

fn sample_values() -> Vec<Value> {
    let mut list = List::new();
    list.push(Value::Int(1));
    list.push(Value::string("two"));

    let mut map = HashMap::new();
    map.insert(Value::string("key"), Value::Double(3.5));
    // Non-string map keys survive the serde single-track roundtrip.
    map.insert(Value::Int(7), Value::Bool(true));

    vec![
        Value::Decimal128(Decimal128Value::from_i64(12345)),
        Value::Date(DateValue {
            year: 2026,
            month: 8,
            day: 15,
        }),
        Value::DateTime(DateTimeValue {
            year: 2026,
            month: 8,
            day: 15,
            hour: 10,
            minute: 30,
            sec: 45,
            microsec: 123456,
        }),
        Value::list(list),
        Value::map(map),
        Value::vector(vec![1.0, 2.0, 3.0]),
        Value::Uuid(UuidValue([7u8; 16])),
        Value::string("restore-me"),
        Value::Null(NullType::Null),
        Value::struct_(vec![
            ("city".to_string(), Value::string("shanghai")),
            (
                "geo".to_string(),
                Value::struct_(vec![("lat".to_string(), Value::Double(31.2))]),
            ),
        ]),
        Value::array(vec![Value::Double(1.0), Value::Double(2.0)]),
    ]
}

fn vertex_undo(value: Value) -> UndoLogEntry {
    UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
        v_label: 1u32,
        vid: VertexId::from_int64(42),
        col_id: ColumnId(0),
        old_value: value,
    })
}

fn edge_undo(value: Value) -> UndoLogEntry {
    UndoLogEntry::UpdateEdgeProp(UpdateEdgePropUndo {
        src_label: 1u32,
        src_vid: VertexId::from_int64(42),
        dst_label: 1u32,
        dst_vid: VertexId::from_int64(43),
        edge_label: 2u32,
        rank: 0,
        col_id: ColumnId(0),
        old_value: value,
    })
}

#[test]
fn undo_roundtrip_restores_original_value() {
    let target = RecordingUndoTarget::new();
    let target = Arc::new(target);

    for value in sample_values() {
        let target = Arc::clone(&target);
        for entry in [vertex_undo(value.clone()), edge_undo(value.clone())] {
            let encoded = postcard::to_allocvec(&entry).expect("encode undo entry");
            let decoded: UndoLogEntry = postcard::from_bytes(&encoded).expect("decode undo entry");
            decoded.undo(&*target, 99).expect("undo should succeed");
        }
    }

    let restored = target.restored();
    let expected: Vec<Value> = sample_values();
    // Each sample value is restored twice (vertex + edge), in order.
    let mut flattened = Vec::new();
    for value in &expected {
        flattened.push(value.clone());
        flattened.push(value.clone());
    }
    assert_eq!(restored.len(), flattened.len());
    for (restored, original) in restored.iter().zip(flattened.iter()) {
        assert_eq!(
            restored, original,
            "undo must restore the original value, got: {:?}",
            restored
        );
    }
}

#[test]
fn undo_entry_old_value_is_never_silently_null() {
    for value in sample_values() {
        if value.is_null() {
            continue;
        }
        let entry = vertex_undo(value.clone());
        let decoded: UndoLogEntry =
            postcard::from_bytes(&postcard::to_allocvec(&entry).expect("encode")).expect("decode");
        let UndoLogEntry::UpdateVertexProp(undo) = decoded else {
            panic!("expected UpdateVertexProp");
        };
        assert!(
            !undo.old_value.is_null(),
            "old_value must not degrade to Null: original {:?}",
            value
        );
    }
}

#[test]
fn failing_undo_returns_error() {
    struct FailingTarget;

    impl UndoTarget for FailingTarget {
        fn undo_update_vertex_property(
            &self,
            _vertex: VertexIdentifier,
            _col_id: ColumnId,
            _value: Value,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Err(UndoLogError::UndoFailed("simulated failure".to_string()))
        }

        fn undo_update_edge_property(
            &self,
            _edge_id: EdgeIdentifier,
            _col_id: ColumnId,
            _value: Value,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Err(UndoLogError::UndoFailed("simulated failure".to_string()))
        }

        fn delete_vertex_type(&self, _label: LabelId) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_edge_type(
            &self,
            _edge_key: graphdb_core::core::types::EdgeKey,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_edge(
            &self,
            _edge_ctx: graphdb_core::core::types::EdgeDeletionContext,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_vertex(
            &self,
            _vertex: VertexIdentifier,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge(
            &self,
            _edge_ctx: graphdb_core::core::types::EdgeDeletionContext,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_vertex_properties(
            &self,
            _label_name: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge_label(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_rename_vertex_properties(
            &self,
            _label_name: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_rename_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }
    }

    let entry = vertex_undo(Value::vector(vec![9.0]));
    assert!(entry.undo(&FailingTarget, 1).is_err());
}
