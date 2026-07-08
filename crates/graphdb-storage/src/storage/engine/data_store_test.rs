#[cfg(test)]
mod tests {
    use crate::storage::engine::data_store::{EdgeTableKey, GraphDataStore};

    #[test]
    fn test_data_store_new_is_empty() {
        let ds = GraphDataStore::new();
        assert!(ds.vertex_tables().read().is_empty());
        assert!(ds.edge_tables().read().is_empty());
        assert!(ds.vertex_label_names().read().is_empty());
        assert!(ds.edge_label_names().read().is_empty());
        assert_eq!(*ds.vertex_label_counter().read(), 0);
        assert_eq!(*ds.edge_label_counter().read(), 0);
    }

    #[test]
    fn test_vertex_label_name_mapping() {
        let ds = GraphDataStore::new();

        ds.vertex_label_names()
            .write()
            .insert("Person".to_string(), 0);
        ds.vertex_label_names()
            .write()
            .insert("Company".to_string(), 1);

        assert_eq!(ds.vertex_label_names().read().get("Person"), Some(&0));
        assert_eq!(ds.vertex_label_names().read().get("Company"), Some(&1));
        assert!(ds.vertex_label_names().read().get("Unknown").is_none());
    }

    #[test]
    fn test_edge_label_name_mapping() {
        let ds = GraphDataStore::new();

        ds.edge_label_names().write().insert("KNOWS".to_string(), 0);
        ds.edge_label_names()
            .write()
            .insert("WORKS_AT".to_string(), 1);

        assert_eq!(ds.edge_label_names().read().get("KNOWS"), Some(&0));
        assert_eq!(ds.edge_label_names().read().get("WORKS_AT"), Some(&1));
    }

    #[test]
    fn test_vertex_label_counter_increment() {
        let ds = GraphDataStore::new();

        {
            let mut counter = ds.vertex_label_counter().write();
            assert_eq!(*counter, 0);
            *counter = 1;
        }
        assert_eq!(*ds.vertex_label_counter().read(), 1);

        {
            let mut counter = ds.vertex_label_counter().write();
            *counter += 1;
        }
        assert_eq!(*ds.vertex_label_counter().read(), 2);
    }

    #[test]
    fn test_edge_label_counter_increment() {
        let ds = GraphDataStore::new();

        {
            let mut counter = ds.edge_label_counter().write();
            assert_eq!(*counter, 0);
            *counter = 5;
        }
        assert_eq!(*ds.edge_label_counter().read(), 5);
    }

    #[test]
    fn test_edge_table_key_creation() {
        let key = EdgeTableKey::new(1, 2, 3);
        assert_eq!(key.src_label, 1);
        assert_eq!(key.dst_label, 2);
        assert_eq!(key.edge_label, 3);

        let from_tuple: EdgeTableKey = (4, 5, 6).into();
        assert_eq!(from_tuple.src_label, 4);
        assert_eq!(from_tuple.dst_label, 5);
        assert_eq!(from_tuple.edge_label, 6);
    }

    #[test]
    fn test_edge_table_key_equality() {
        let a = EdgeTableKey::new(1, 2, 3);
        let b = EdgeTableKey::new(1, 2, 3);
        let c = EdgeTableKey::new(1, 2, 4);

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_edge_table_key_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let key = EdgeTableKey::new(10, 20, 30);
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let hash1 = hasher.finish();

        let key2 = EdgeTableKey::new(10, 20, 30);
        let mut hasher2 = DefaultHasher::new();
        key2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_data_store_default_is_empty() {
        let ds = GraphDataStore::default();
        assert!(ds.vertex_tables().read().is_empty());

        // Default should behave the same as new()
        let ds2 = GraphDataStore::new();
        assert_eq!(
            *ds.vertex_label_counter().read(),
            *ds2.vertex_label_counter().read()
        );
    }

    #[test]
    fn test_add_and_retrieve_vertex_table() {
        let ds = GraphDataStore::new();

        // Create a VertexTable and insert it into the data store
        let label_id = 0;
        let table = crate::storage::vertex::VertexTable::new(
            label_id,
            "Person".to_string(),
            crate::storage::vertex::VertexSchema {
                label_id,
                label_name: "Person".to_string(),
                properties: vec![],
                primary_key_index: 0,
                schema_version: 1,
            },
        );
        ds.vertex_tables().write().insert(label_id, table);

        assert!(ds.vertex_tables().read().contains_key(&label_id));
        assert_eq!(ds.vertex_tables().read().len(), 1);
    }

    #[test]
    fn test_remove_vertex_table() {
        let ds = GraphDataStore::new();

        let label_id = 0;
        let table = crate::storage::vertex::VertexTable::new(
            label_id,
            "Temp".to_string(),
            crate::storage::vertex::VertexSchema {
                label_id,
                label_name: "Temp".to_string(),
                properties: vec![],
                primary_key_index: 0,
                schema_version: 1,
            },
        );
        ds.vertex_tables().write().insert(label_id, table);
        assert_eq!(ds.vertex_tables().read().len(), 1);

        ds.vertex_tables().write().remove(&label_id);
        assert!(ds.vertex_tables().read().is_empty());
    }

    #[test]
    fn test_data_store_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<GraphDataStore>();
        assert_sync::<GraphDataStore>();
    }
}
