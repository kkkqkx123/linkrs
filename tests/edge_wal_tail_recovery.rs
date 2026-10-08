use std::collections::HashMap;

use linkrs::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs::core::vertex_edge_path::Tag;
use linkrs::core::{DataType, Edge, Value, Vertex};
use linkrs::storage::{
    GraphStorage, PersistenceConfig, StoragePersistenceOps, StorageReader, StorageSchemaOps,
    StorageWriter,
};

fn config_for(path: std::path::PathBuf) -> PersistenceConfig {
    // Background checkpointing and async WAL flush are disabled so the
    // commit-order matrix has a deterministic fence: with them on, a
    // checkpoint can land between the write being issued and the process
    // "crashing", which changes what the tail replay has to redo.
    let mut cfg = PersistenceConfig::for_work_dir(&path);
    cfg.async_checkpoint_enabled = false;
    cfg.wal_enable_async_flush = false;
    cfg
}

fn build_10k(path: std::path::PathBuf) {
    let mut storage =
        GraphStorage::new_with_persistence(path.clone(), config_for(path)).expect("storage");
    let mut space = SpaceInfo::new("s".to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("space");
    storage
        .create_tag(
            "s",
            &TagInfo::new("N".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    storage
        .create_edge_type(
            "s",
            &EdgeTypeInfo::new("L".to_string())
                .with_src_tag("N".to_string())
                .with_dst_tag("N".to_string()),
        )
        .expect("edge type");
    let vertices: Vec<Vertex> = (0..2000)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("vid"),
                Tag::new(
                    "N".to_string(),
                    [("value".to_string(), Value::BigInt(i as i64))]
                        .into_iter()
                        .map(|(name, value)| (std::sync::Arc::from(name.as_str()), value))
                        .collect(),
                ),
            )
        })
        .collect();
    storage.batch_insert_vertices("s", vertices).expect("verts");
    let edges: Vec<Edge> = (0..10000)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i % 2000) as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 1) % 2000) as i64).expect("vid"),
            edge_type: "L".to_string(),
            ranking: (i / 2000) as i64,
            props: HashMap::new(),
        })
        .collect();
    storage.batch_insert_edges("s", edges).expect("edges");
    storage.create_checkpoint().expect("checkpoint");
}

#[test]
fn wal_synced_tail_survives_unclean_drop() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();
    build_10k(path.clone());
    {
        let mut storage =
            GraphStorage::open_with_persistence_config(path.clone(), config_for(path.clone()))
                .expect("reopen1");
        let tail: Vec<Edge> = (0..1000)
            .map(|i| Edge {
                src: VertexId::try_from_int64((i % 2000) as i64).expect("vid"),
                dst: VertexId::try_from_int64(((i + 2) % 2000) as i64).expect("vid"),
                edge_type: "L".to_string(),
                ranking: 0,
                props: HashMap::new(),
            })
            .collect();
        storage.batch_insert_edges("s", tail).expect("tail");
        storage.flush().expect("sync");
    }
    let reopened =
        GraphStorage::open_with_persistence_config(path.clone(), config_for(path.clone()))
            .expect("reopen after unclean drop");
    let edges = reopened.scan_edges_by_type("s", "L").expect("audit scan");
    assert_eq!(edges.len(), 11000, "flushed tail must replay after crash");
}
