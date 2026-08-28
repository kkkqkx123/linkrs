#[cfg(test)]
mod tests {
    use graphdb_core::{DataType, Value};
    use crate::engine::graph_storage::GraphStorageContext;
    use crate::types::StoragePropertyDef;
    use tempfile::TempDir;

    fn temp_dir(name: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(&format!("graphdb_persistence_{name}_"))
            .tempdir()
            .expect("temporary persistence directory should be created")
    }

    #[test]
    fn test_flush_tables_to_dir_custom_path() {
        let dir = temp_dir("flush_custom");
        let data_dir = dir.path().join("custom_data");

        let graph = GraphStorageContext::new();

        let person_label = graph
            .create_vertex_type(
                "person",
                vec![StoragePropertyDef::new(
                    "name".to_string(),
                    DataType::String,
                )],
                "name",
            )
            .unwrap();

        graph
            .insert_vertex(
                person_label,
                "alice",
                &[("name".to_string(), Value::string("Alice"))],
                100,
            )
            .unwrap();

        // Flush to custom dir
        graph.flush_tables_to_dir(&data_dir).unwrap();

        assert!(data_dir.join("vertices").exists());
        assert!(data_dir.join("edges").exists());
    }
}
