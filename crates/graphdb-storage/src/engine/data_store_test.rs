#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::edge::{EdgeSchema, EdgeStore, EdgeStrategy};
    use crate::engine::data_store::{EdgeTableKey, GraphDataStore};
    use crate::vertex::{ShardedVertexTable, VertexSchema};
    use graphdb_core::{StorageError, StorageResult};

    /// Test-only extension trait for invariant verification.
    trait VerifyInvariants {
        fn verify_invariants(&self) -> StorageResult<()>;
    }

    impl VerifyInvariants for GraphDataStore {
        fn verify_invariants(&self) -> StorageResult<()> {
            let vertex_names = self.read_vertex_label_names();
            let edge_names = self.read_edge_label_names();
            let vertex_counter = self.read_vertex_counter();
            let edge_counter = self.read_edge_counter();
            let vertex_tables = self.read_vertex_tables();
            let edge_tables = self.read_edge_tables();
            let edge_index = self.test_read_edge_label_index();

            for (name, label) in &*vertex_names {
                if !vertex_tables.contains_key(label) {
                    return Err(StorageError::invalid_operation(format!(
                        "vertex label '{}' points to missing table {}",
                        name, label
                    )));
                }
            }
            for (name, label) in &*edge_names {
                if !edge_index.contains_key(label) {
                    return Err(StorageError::invalid_operation(format!(
                        "edge label '{}' has no indexed partitions",
                        name
                    )));
                }
            }
            for (key, table_arc) in &*edge_tables {
                if !edge_index
                    .get(&key.edge_label)
                    .is_some_and(|keys| keys.contains(key))
                {
                    return Err(StorageError::invalid_operation(format!(
                        "edge table {:?} is missing from reverse index",
                        key
                    )));
                }
                let table = table_arc.read();
                if table.schema().label_id != key.edge_label {
                    return Err(StorageError::invalid_operation(format!(
                        "edge table {:?} has mismatched schema label",
                        key
                    )));
                }
            }
            for table in vertex_tables.values() {
                table.verify_invariants()?;
            }
            if vertex_tables.keys().any(|label| *label >= *vertex_counter)
                || edge_index.keys().any(|label| *label >= *edge_counter)
            {
                return Err(StorageError::invalid_operation(
                    "label counter does not cover registered labels".to_string(),
                ));
            }
            Ok(())
        }
    }

    fn vertex_table(label: u32, name: &str) -> ShardedVertexTable {
        let schema = VertexSchema {
            label_id: label,
            label_name: name.to_string(),
            properties: Vec::new(),
            primary_key_index: 0,
            schema_version: 1,
        };
        ShardedVertexTable::new(label, name.to_string(), schema)
    }

    fn edge_table(label: u32, src_label: u32, dst_label: u32, name: &str) -> EdgeStore {
        EdgeStore::new(EdgeSchema {
            label_id: label,
            label_name: name.to_string(),
            src_label,
            dst_label,
            properties: Vec::new(),
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        })
        .expect("edge table should be valid")
    }

    #[test]
    fn new_catalog_is_empty_and_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        let catalog = GraphDataStore::new();
        assert_eq!(catalog.catalog_counts(), (0, 0));
        catalog
            .verify_invariants()
            .expect("empty catalog invariants should hold");
        assert_send::<GraphDataStore>();
        assert_sync::<GraphDataStore>();
    }

    #[test]
    fn registration_updates_names_counters_tables_and_reverse_index() {
        let catalog = GraphDataStore::new();
        let person = catalog
            .register_vertex_type("Person".to_string(), None, |label| {
                Ok(vertex_table(label, "Person"))
            })
            .expect("vertex registration should succeed");
        let company = catalog
            .register_vertex_type("Company".to_string(), Some(7), |label| {
                Ok(vertex_table(label, "Company"))
            })
            .expect("explicit vertex registration should succeed");
        let next = catalog
            .register_vertex_type("City".to_string(), None, |label| {
                Ok(vertex_table(label, "City"))
            })
            .expect("counter should advance beyond explicit labels");
        let works_at = catalog
            .register_edge_type("WORKS_AT".to_string(), None, person, company, |label| {
                Ok(edge_table(label, person, company, "WORKS_AT"))
            })
            .expect("edge registration should succeed");

        assert_eq!(catalog.vertex_label_id("Person"), Some(person));
        assert_eq!(catalog.vertex_label_id("Company"), Some(company));
        assert_eq!(catalog.edge_label_id("WORKS_AT"), Some(works_at));
        assert_eq!(next, 8);
        assert_eq!(catalog.catalog_counts(), (3, 1));
        catalog
            .verify_invariants()
            .expect("registered catalog invariants should hold");
    }

    #[test]
    fn dropping_vertex_atomically_removes_dependent_edge_entries() {
        let catalog = GraphDataStore::new();
        let person = catalog
            .register_vertex_type("Person".to_string(), None, |label| {
                Ok(vertex_table(label, "Person"))
            })
            .expect("vertex registration should succeed");
        let company = catalog
            .register_vertex_type("Company".to_string(), None, |label| {
                Ok(vertex_table(label, "Company"))
            })
            .expect("vertex registration should succeed");
        catalog
            .register_edge_type("WORKS_AT".to_string(), None, person, company, |label| {
                Ok(edge_table(label, person, company, "WORKS_AT"))
            })
            .expect("edge registration should succeed");

        catalog
            .drop_vertex_type("Person")
            .expect("vertex drop should succeed");

        assert_eq!(catalog.vertex_label_id("Person"), None);
        assert_eq!(catalog.edge_label_id("WORKS_AT"), None);
        assert_eq!(catalog.catalog_counts(), (1, 0));
        catalog
            .verify_invariants()
            .expect("drop should preserve catalog invariants");
    }

    #[test]
    fn concurrent_partition_creation_registers_one_reverse_index_entry() {
        let catalog = Arc::new(GraphDataStore::new());
        let edge_label = catalog
            .register_edge_type("REL".to_string(), None, 0, 0, |label| {
                Ok(edge_table(label, 0, 0, "REL"))
            })
            .expect("template edge registration should succeed");

        let mut workers = Vec::new();
        for _ in 0..8 {
            let catalog = catalog.clone();
            workers.push(std::thread::spawn(move || {
                let key = EdgeTableKey::new(1, 2, edge_label);
                let template = EdgeTableKey::new(0, 0, edge_label);
                catalog
                    .with_edge_partition_mut(
                        key,
                        template,
                        |table| {
                            let mut schema = table.schema().clone();
                            schema.src_label = 1;
                            schema.dst_label = 2;
                            EdgeStore::new(schema)
                        },
                        |_| Ok(()),
                    )
                    .expect("partition creation should succeed");
            }));
        }
        for worker in workers {
            worker.join().expect("partition worker should not panic");
        }

        assert_eq!(catalog.catalog_counts(), (0, 2));
        catalog
            .verify_invariants()
            .expect("concurrent partition creation should preserve invariants");
    }

    #[test]
    fn thirty_two_thread_schema_and_data_mix_has_no_lock_cycle() {
        let catalog = Arc::new(GraphDataStore::new());
        for index in 0..16 {
            let name = format!("Node{}", index);
            catalog
                .register_vertex_type(name.clone(), None, |label| Ok(vertex_table(label, &name)))
                .expect("initial schema registration should succeed");
        }

        let barrier = Arc::new(std::sync::Barrier::new(32));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        for worker_id in 0..32 {
            let catalog = catalog.clone();
            let barrier = barrier.clone();
            let done_tx = done_tx.clone();
            std::thread::spawn(move || {
                barrier.wait();
                if worker_id < 16 {
                    let label = worker_id as u32;
                    catalog
                        .with_vertex_table_mut(label, |table| {
                            let _ = table.total_count();
                            Ok(())
                        })
                        .expect("data operation should complete");
                } else {
                    let name = format!("Node{}", worker_id - 16);
                    assert!(catalog.vertex_label_id_for_name(&name).is_some());
                }
                done_tx.send(()).expect("watchdog channel should be open");
            });
        }
        drop(done_tx);

        for _ in 0..32 {
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("schema/data mix must not deadlock");
        }
    }
}
