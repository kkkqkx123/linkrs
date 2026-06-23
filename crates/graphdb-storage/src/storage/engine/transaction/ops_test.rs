#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::core::types::{LabelId, VertexId};
    use crate::core::Value;
    use crate::storage::edge::{EdgeSchema, EdgeStrategy, EdgeTable};
    use crate::storage::engine::data_store::EdgeTableKey;
    use crate::storage::types::StoragePropertyDef;
    use crate::storage::vertex::{VertexSchema, VertexTable};

    use crate::storage::engine::transaction::ops::{
        AddEdgeParams, DeleteEdgeParams, DeleteEdgeTypeParams, RevertDeleteEdgeParams,
        TransactionOps,
    };

    fn create_vertex_table(label: LabelId, name: &str) -> VertexTable {
        let schema = VertexSchema {
            label_id: label,
            label_name: name.to_string(),
            properties: vec![
                StoragePropertyDef::new("name".to_string(), crate::core::DataType::String),
                StoragePropertyDef::new("age".to_string(), crate::core::DataType::BigInt),
            ],
            primary_key_index: 0,
            schema_version: 1,
        };
        VertexTable::new(label, name.to_string(), schema)
    }

    fn create_edge_table(
        edge_label: LabelId,
        src_label: LabelId,
        dst_label: LabelId,
        name: &str,
    ) -> EdgeTable {
        let schema = EdgeSchema {
            label_id: edge_label,
            label_name: name.to_string(),
            src_label,
            dst_label,
            properties: vec![StoragePropertyDef::new(
                "since".to_string(),
                crate::core::DataType::Int,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeTable::new(schema).expect("Failed to create EdgeTable")
    }

    #[test]
    #[allow(clippy::useless_vec)]
    fn test_add_vertex_int_id() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        let vid = VertexId::from_int64(100);
        let properties = vec![
            ("name".to_string(), Value::String("Alice".to_string())),
            ("age".to_string(), Value::BigInt(30)),
        ];
        let props_bytes: Vec<(String, Vec<u8>)> = properties
            .iter()
            .map(|(k, v)| (k.clone(), crate::transaction::codec::value_to_bytes(v)))
            .collect();

        let result = TransactionOps::add_vertex(&mut vertex_tables, 0, vid, &props_bytes, 1);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), VertexId::from_int64(0));

        let table = vertex_tables.get(&0).unwrap();
        let internal = table.get_internal_id_by_i64(100, 1);
        assert!(internal.is_some());
    }

    #[test]
    #[allow(clippy::useless_vec)]
    fn test_add_vertex_string_id() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        // Use a string ID that is NOT 8 bytes (avoids as_int64() collision)
        let vid = VertexId::from_string("user-alice");
        let properties = vec![
            ("name".to_string(), Value::String("Alice".to_string())),
            ("age".to_string(), Value::BigInt(30)),
        ];
        let props_bytes: Vec<(String, Vec<u8>)> = properties
            .iter()
            .map(|(k, v)| (k.clone(), crate::transaction::codec::value_to_bytes(v)))
            .collect();

        let result = TransactionOps::add_vertex(&mut vertex_tables, 0, vid, &props_bytes, 1);
        assert!(result.is_ok());

        let table = vertex_tables.get(&0).unwrap();
        let internal = table.get_internal_id("user-alice", 1);
        assert!(internal.is_some());
    }

    #[test]
    fn test_add_vertex_label_not_found() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();

        let vid = VertexId::from_int64(1);
        let result = TransactionOps::add_vertex(&mut vertex_tables, 99, vid, &[], 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_edge() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));
        vertex_tables.insert(1, create_vertex_table(1, "Person"));

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        edge_tables.insert(
            EdgeTableKey::new(0, 1, 0),
            create_edge_table(0, 0, 1, "KNOWS"),
        );

        let vid1 = VertexId::from_int64(100);
        let vid2 = VertexId::from_int64(101);

        TransactionOps::add_vertex(&mut vertex_tables, 0, vid1, &[], 1).unwrap();
        TransactionOps::add_vertex(&mut vertex_tables, 1, vid2, &[], 1).unwrap();

        let src_internal = vertex_tables
            .get(&0)
            .unwrap()
            .get_internal_id_by_i64(100, 1)
            .unwrap();
        let dst_internal = vertex_tables
            .get(&1)
            .unwrap()
            .get_internal_id_by_i64(101, 1)
            .unwrap();

        let params = AddEdgeParams {
            src_label: 0,
            src_vid: src_internal,
            dst_label: 1,
            dst_vid: dst_internal,
            edge_label: 0,
            rank: 0,
        };

        let result = TransactionOps::add_edge(&mut edge_tables, &vertex_tables, params, &[], 1);
        assert!(result.is_ok(), "add_edge failed: {:?}", result.err());
    }

    #[test]
    fn test_add_edge_missing_src_label() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        edge_tables.insert(
            EdgeTableKey::new(0, 1, 0),
            create_edge_table(0, 0, 1, "KNOWS"),
        );

        let params = AddEdgeParams {
            src_label: 0,
            src_vid: 0,
            dst_label: 1,
            dst_vid: 0,
            edge_label: 0,
            rank: 0,
        };

        let result = TransactionOps::add_edge(&mut edge_tables, &vertex_tables, params, &[], 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_vertex_id() {
        let mut table = create_vertex_table(0, "Person");
        table
            .insert_by_i64(
                100,
                &[("name".to_string(), Value::String("Alice".to_string()))],
                1,
            )
            .unwrap();
        table
            .insert(
                "user-bob-ext",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                1,
            )
            .unwrap();

        let resolved_int = TransactionOps::resolve_vertex_id(&table, VertexId::from_int64(100), 1);
        assert_eq!(resolved_int, Some(0));

        let resolved_str =
            TransactionOps::resolve_vertex_id(&table, VertexId::from_string("user-bob-ext"), 1);
        assert_eq!(resolved_str, Some(1));

        let not_found = TransactionOps::resolve_vertex_id(&table, VertexId::from_int64(999), 1);
        assert_eq!(not_found, None);
    }

    #[test]
    fn test_delete_vertex() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        TransactionOps::add_vertex(&mut vertex_tables, 0, VertexId::from_int64(1), &[], 1).unwrap();

        let result =
            TransactionOps::delete_vertex(&mut vertex_tables, 0, VertexId::from_int64(1), 2);
        assert!(result.is_ok());

        // After deletion, the vertex should not be visible at timestamp 2
        let table = vertex_tables.get(&0).unwrap();
        let v = table.get_by_internal_id(0, 2);
        assert!(v.is_none());
    }

    #[test]
    fn test_update_vertex_property_by_vid() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        // Insert with external ID = 1 (int64 vid), gets internal ID 0
        let vid = VertexId::from_int64(1);
        TransactionOps::add_vertex(
            &mut vertex_tables,
            0,
            vid,
            &[(
                "name".to_string(),
                crate::transaction::codec::value_to_bytes(&Value::String("Alice".to_string())),
            )],
            1,
        )
        .unwrap();

        // update_vertex_property_by_vid looks up external ID in the table
        let result = TransactionOps::update_vertex_property_by_vid(
            &mut vertex_tables,
            0,
            VertexId::from_int64(1),
            "name",
            &Value::String("AliceUpdated".to_string()),
            2,
        );
        assert!(result.is_ok());

        let table = vertex_tables.get(&0).unwrap();
        let record = table.get_by_internal_id(0, 2).unwrap();
        let name_val = record
            .properties
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v);
        assert_eq!(name_val, Some(&Value::String("AliceUpdated".to_string())));
    }

    #[test]
    fn test_delete_vertex_type_cascades_to_edge_types() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));
        vertex_tables.insert(1, create_vertex_table(1, "Employee"));

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        edge_tables.insert(
            EdgeTableKey::new(0, 0, 0),
            create_edge_table(0, 0, 0, "KNOWS"),
        );
        edge_tables.insert(
            EdgeTableKey::new(0, 1, 1),
            create_edge_table(1, 0, 1, "WORKS_AT"),
        );

        let mut vertex_label_names: HashMap<String, LabelId> = HashMap::new();
        vertex_label_names.insert("Person".to_string(), 0);
        vertex_label_names.insert("Employee".to_string(), 1);

        let mut edge_label_names: HashMap<String, LabelId> = HashMap::new();
        edge_label_names.insert("KNOWS".to_string(), 0);
        edge_label_names.insert("WORKS_AT".to_string(), 1);

        TransactionOps::delete_vertex_type(
            &mut vertex_tables,
            &mut edge_tables,
            &mut vertex_label_names,
            &mut edge_label_names,
            1,
        )
        .unwrap();

        assert!(!vertex_tables.contains_key(&1));
        assert!(!vertex_label_names.contains_key("Employee"));
        // WORKS_AT edge type should be removed because its src or dst label is gone
        assert!(!edge_label_names.contains_key("WORKS_AT"));
        // KNOWS (0,0,0) should still exist since neither src=0 nor dst=0 was removed
        assert!(edge_tables.contains_key(&EdgeTableKey::new(0, 0, 0)));
    }

    #[test]
    fn test_delete_edge_type() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));
        vertex_tables.insert(1, create_vertex_table(1, "Person"));

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        edge_tables.insert(
            EdgeTableKey::new(0, 1, 0),
            create_edge_table(0, 0, 1, "KNOWS"),
        );

        let mut edge_label_names: HashMap<String, LabelId> = HashMap::new();
        edge_label_names.insert("KNOWS".to_string(), 0);

        let params = DeleteEdgeTypeParams {
            src_label: 0,
            dst_label: 1,
            edge_label: 0,
        };

        TransactionOps::delete_edge_type(&mut edge_tables, &mut edge_label_names, params).unwrap();

        assert!(!edge_label_names.contains_key("KNOWS"));
        assert!(!edge_tables.contains_key(&EdgeTableKey::new(0, 1, 0)));
    }

    #[test]
    fn test_revert_delete_vertex() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        TransactionOps::add_vertex(
            &mut vertex_tables,
            0,
            VertexId::from_int64(1),
            &[(
                "name".to_string(),
                crate::transaction::codec::value_to_bytes(&Value::String("Alice".to_string())),
            )],
            1,
        )
        .unwrap();

        TransactionOps::delete_vertex(&mut vertex_tables, 0, VertexId::from_int64(1), 2).unwrap();

        // Revert at the same timestamp as deletion (or earlier)
        let result =
            TransactionOps::revert_delete_vertex(&mut vertex_tables, 0, VertexId::from_int64(1), 2);
        assert!(result.is_ok(), "revert_delete_vertex failed: {:?}", result);

        let table = vertex_tables.get(&0).unwrap();
        let record = table.get_by_internal_id(0, 3);
        assert!(record.is_some());
    }

    #[test]
    fn test_revert_delete_edge() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        vertex_tables.insert(0, create_vertex_table(0, "Person"));

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        edge_tables.insert(
            EdgeTableKey::new(0, 0, 0),
            create_edge_table(0, 0, 0, "KNOWS"),
        );

        TransactionOps::add_vertex(&mut vertex_tables, 0, VertexId::from_int64(1), &[], 1).unwrap();
        TransactionOps::add_vertex(&mut vertex_tables, 0, VertexId::from_int64(2), &[], 1).unwrap();

        let add_params = AddEdgeParams {
            src_label: 0,
            src_vid: 0,
            dst_label: 0,
            dst_vid: 1,
            edge_label: 0,
            rank: 0,
        };
        TransactionOps::add_edge(&mut edge_tables, &vertex_tables, add_params, &[], 1).unwrap();

        let del_params = DeleteEdgeParams {
            src_label: 0,
            src_vid: 0,
            dst_label: 0,
            dst_vid: 1,
            edge_label: 0,
            rank: 0,
        };
        TransactionOps::delete_edge(&mut edge_tables, del_params, 0i32, 0i32, 2).unwrap();

        let revert_params = RevertDeleteEdgeParams {
            src_label: 0,
            dst_label: 0,
            edge_label: 0,
            src_vid: 0,
            dst_vid: 1,
            rank: 0,
        };
        let result = TransactionOps::revert_delete_edge(
            &mut edge_tables,
            revert_params,
            0i32,
            0i32,
            3,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_vertex_type_undo() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        let mut vertex_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut vertex_label_counter: LabelId = 0;

        TransactionOps::create_vertex_type_undo(
            &mut vertex_tables,
            &mut vertex_label_names,
            &mut vertex_label_counter,
            "Person",
        )
        .unwrap();

        assert!(vertex_tables.contains_key(&0));
        assert_eq!(vertex_label_names.get("Person"), Some(&0));
        assert!(vertex_label_counter >= 1);

        // Create another
        TransactionOps::create_vertex_type_undo(
            &mut vertex_tables,
            &mut vertex_label_names,
            &mut vertex_label_counter,
            "Employee",
        )
        .unwrap();

        assert!(vertex_tables.contains_key(&1));
        assert_eq!(vertex_label_names.get("Employee"), Some(&1));
    }

    #[test]
    fn test_create_edge_type_undo() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        let mut vertex_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut vertex_label_counter: LabelId = 0;

        TransactionOps::create_vertex_type_undo(
            &mut vertex_tables,
            &mut vertex_label_names,
            &mut vertex_label_counter,
            "Person",
        )
        .unwrap();

        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        let mut edge_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut edge_label_counter: LabelId = 0;

        let result = TransactionOps::create_edge_type_undo(
            &mut edge_tables,
            &mut edge_label_names,
            &mut edge_label_counter,
            &vertex_tables,
            "KNOWS",
            "Person",
            "Person",
        );
        assert!(result.is_ok());

        let key = EdgeTableKey::new(0, 0, 0);
        assert!(edge_tables.contains_key(&key));
        assert_eq!(edge_label_names.get("KNOWS"), Some(&0));
    }

    #[test]
    fn test_create_edge_type_undo_missing_vertex_label() {
        let vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        let mut edge_tables: HashMap<EdgeTableKey, EdgeTable> = HashMap::new();
        let mut edge_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut edge_label_counter: LabelId = 0;

        let result = TransactionOps::create_edge_type_undo(
            &mut edge_tables,
            &mut edge_label_names,
            &mut edge_label_counter,
            &vertex_tables,
            "KNOWS",
            "Person",
            "Person",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_revert_rename_vertex_properties() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        let mut vertex_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut vertex_label_counter: LabelId = 0;

        // First create a vertex type with property "name"
        TransactionOps::create_vertex_type_undo(
            &mut vertex_tables,
            &mut vertex_label_names,
            &mut vertex_label_counter,
            "Person",
        )
        .unwrap();

        // Add the "name" property to the schema before renaming
        {
            let table = vertex_tables.get_mut(&0).unwrap();
            table
                .add_property(crate::storage::types::StoragePropertyDef::new(
                    "name".to_string(),
                    crate::core::DataType::String,
                ))
                .unwrap();
        }

        // Rename "name" -> "full_name" in schema
        {
            let table = vertex_tables.get_mut(&0).unwrap();
            table.rename_property("name", "full_name").unwrap();
        }

        let result = TransactionOps::revert_rename_vertex_properties(
            &mut vertex_tables,
            &mut vertex_label_names,
            "Person",
            &["full_name".to_string()],
            &["name".to_string()],
        );
        assert!(result.is_ok());

        let table = vertex_tables.get(&0).unwrap();
        let schema = table.schema();
        assert!(schema.properties.iter().any(|p| p.name == "name"));
    }

    #[test]
    fn test_undo_maintains_vertex_schema_version() {
        let mut vertex_tables: HashMap<LabelId, VertexTable> = HashMap::new();
        let mut vertex_label_names: HashMap<String, LabelId> = HashMap::new();
        let mut vertex_label_counter: LabelId = 0;

        // Create vertex type (version starts at 1)
        TransactionOps::create_vertex_type_undo(
            &mut vertex_tables,
            &mut vertex_label_names,
            &mut vertex_label_counter,
            "Person",
        )
        .unwrap();

        let table = vertex_tables.get(&0).unwrap();
        assert_eq!(table.schema().schema_version, 1);

        // Add a property to increment version (should go to 2)
        {
            let table = vertex_tables.get_mut(&0).unwrap();
            table.add_property(StoragePropertyDef::new(
                "email".to_string(),
                crate::core::DataType::String,
            )).unwrap();
        }

        let table = vertex_tables.get(&0).unwrap();
        assert_eq!(table.schema().schema_version, 2);

        // Rename the property to increment version (should go to 3)
        {
            let table = vertex_tables.get_mut(&0).unwrap();
            table.rename_property("email", "email_address").unwrap();
        }

        let table = vertex_tables.get(&0).unwrap();
        let version_before_undo = table.schema().schema_version;
        assert_eq!(version_before_undo, 3);

        // Undo rename - version should remain the same (not revert to 2)
        // because the undo operation doesn't decrement version
        let result = TransactionOps::revert_rename_vertex_properties(
            &mut vertex_tables,
            &mut vertex_label_names,
            "Person",
            &["email_address".to_string()],
            &["email".to_string()],
        );
        assert!(result.is_ok());

        let table = vertex_tables.get(&0).unwrap();
        // Version should remain at 3 after undo
        // (undo preserves the current version, doesn't revert it)
        assert_eq!(table.schema().schema_version, version_before_undo);

        // Verify the schema was actually reverted (name changed back to email)
        assert!(table.schema().properties.iter().any(|p| p.name == "email"));
        assert!(!table.schema().properties.iter().any(|p| p.name == "email_address"));
    }

}
