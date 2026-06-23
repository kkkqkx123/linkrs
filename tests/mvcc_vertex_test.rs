//! Integration tests for VertexTable MVCC functionality

#[cfg(test)]
mod tests {
    use graphdb_storage::core::{DataType, Value};
    use graphdb_storage::storage::mvcc::MVCCTable;
    use graphdb_storage::storage::{VertexTable, VertexSchema, StoragePropertyDef};

    fn create_test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 0,
            label_name: "person".to_string(),
            properties: vec![
                StoragePropertyDef::new("name".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int,
                    nullable: true,
                    default_value: None,
                },
            ],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn test_mvcc_snapshot_isolation() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Insert initial data at ts=100
        let id1 = table
            .insert(
                "v1",
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ],
                100,
            )
            .unwrap();

        // Register snapshot at ts=100
        let snap1 = table.register_snapshot(100).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Delete at ts=200
        table.delete("v1", 200).unwrap();

        // Snapshot at ts=100 should still see data (before deletion)
        let record_at_snap = table.get_by_internal_id(id1, 100);
        assert!(record_at_snap.is_some(), "Snapshot should see data at ts=100");
        let record = record_at_snap.unwrap();
        assert_eq!(record.properties.len(), 2);
        assert_eq!(
            record
                .properties
                .iter()
                .find(|(n, _)| n == "age")
                .map(|(_, v)| v),
            Some(&Value::Int(30)),
        );

        // Current view should not see deleted data
        assert!(table.get_by_internal_id(id1, 250).is_none(), "Current view should not see deleted data");

        // Unregister snapshot
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
        assert_eq!(table.min_active_snapshot_ts(), u32::MAX);
    }

    #[test]
    fn test_mvcc_multiple_snapshots() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Insert at ts=100
        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();

        // Create snap1 at ts=100
        let snap1 = table.register_snapshot(100).unwrap();
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Insert another vertex at ts=150
        table
            .insert(
                "v2",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                150,
            )
            .unwrap();

        // Create snap2 at ts=200
        let snap2 = table.register_snapshot(200).unwrap();
        assert_eq!(table.active_snapshot_count(), 2);
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Delete v1 at ts=250
        table.delete("v1", 250).unwrap();

        // snap1 sees v1 at ts=100
        assert!(
            table.get_by_internal_id(0, 100).is_some(),
            "snap1 should see v1 at ts=100"
        );

        // snap2 sees v1 at ts=200 (before deletion at 250)
        assert!(
            table.get_by_internal_id(0, 200).is_some(),
            "snap2 should see v1 at ts=200"
        );

        // Unregister snap1
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);
        assert_eq!(table.min_active_snapshot_ts(), 200);

        // Unregister snap2
        table.unregister_snapshot(snap2).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
        assert_eq!(table.min_active_snapshot_ts(), u32::MAX);
    }

    #[test]
    fn test_mvcc_concurrent_same_timestamp() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();

        // Register multiple snapshots at same timestamp
        let snap1 = table.register_snapshot(100).unwrap();
        let snap2 = table.register_snapshot(100).unwrap();
        let snap3 = table.register_snapshot(100).unwrap();

        // All registered at same ts
        assert_eq!(snap1.ts, snap2.ts);
        assert_eq!(snap2.ts, snap3.ts);

        // But have different IDs
        assert_ne!(snap1.id, snap2.id);
        assert_ne!(snap2.id, snap3.id);

        // Count should be 1 (one timestamp)
        assert_eq!(table.active_snapshot_count(), 1);

        // Unregister one
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);

        // Unregister another
        table.unregister_snapshot(snap2).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);

        // Unregister last
        table.unregister_snapshot(snap3).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
    }

    #[test]
    fn test_mvcc_through_trait() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();

        // Use MVCCTable trait methods
        let snap = <VertexTable as MVCCTable>::register_snapshot(&mut table, 100).unwrap();
        assert_eq!(
            <VertexTable as MVCCTable>::active_snapshot_count(&table),
            1
        );

        <VertexTable as MVCCTable>::unregister_snapshot(&mut table, snap).unwrap();
        assert_eq!(
            <VertexTable as MVCCTable>::active_snapshot_count(&table),
            0
        );

        // GC should be a no-op for VertexTable
        let gc_count = <VertexTable as MVCCTable>::gc(&mut table, 200).unwrap();
        assert_eq!(gc_count, 0);
    }
}
