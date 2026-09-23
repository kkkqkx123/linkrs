mod auth_admin_tests;
mod batch_window_tests;
mod context_cursor_tests;
mod edge_tests;
mod freeze_compaction_tests;
mod group_window_tests;
mod layout_version_tests;
mod schema_tests;
mod serial_tests;
mod string_id_tests;
mod vertex_tests;

use crate::{
    GraphStorage, PersistenceConfig, PropertyGraphConfig, ScanOptions, StorageAdmin,
    StorageAuthOps, StorageCommitOps, StorageOperationContext, StorageOperationContextOps,
    StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};
use graphdb_core::types::{
    AutoCompactConfig, EdgeTypeInfo, Index, IndexConfig, IndexField, IndexType, PropertyDef,
    SpaceInfo, Timestamp, UserInfo, VertexId,
};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::DataType;
use graphdb_core::{Edge, EdgeDirection, RoleType, Value, Vertex};

pub(super) fn create_test_storage() -> GraphStorage {
    GraphStorage::new().expect("Failed to create GraphStorage")
}

pub(super) fn create_persistent_storage() -> (tempfile::TempDir, GraphStorage) {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let storage = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
        .expect("Failed to create persistent GraphStorage");
    (temp_dir, storage)
}

pub(super) fn setup_space(storage: &mut GraphStorage) -> u64 {
    let mut space = SpaceInfo::new("test_space".to_string())
        .with_vid_type(DataType::BigInt)
        .with_comment(Some("test".to_string()));
    storage.create_space(&mut space).unwrap();
    storage.get_space_id("test_space").unwrap()
}

pub(super) fn setup_person_tag(storage: &mut GraphStorage) -> u32 {
    // The first property is the primary key, which mirrors the external
    // vertex id: `id` must equal the vid on every write.
    let tag = graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
        PropertyDef::new("id".to_string(), DataType::BigInt),
        PropertyDef::new("name".to_string(), DataType::String),
        PropertyDef::new("age".to_string(), DataType::BigInt),
    ]);
    storage
        .create_tag("test_space", &tag)
        .expect("Failed to create tag")
}

pub(super) fn setup_knows_edge(storage: &mut GraphStorage) -> u32 {
    let edge = EdgeTypeInfo::new("KNOWS".to_string())
        .with_properties(vec![PropertyDef::new("since".to_string(), DataType::Int)]);
    storage
        .create_edge_type("test_space", &edge)
        .expect("Failed to create edge type")
}

pub(super) fn insert_test_vertex(storage: &mut GraphStorage, id: i64, name: &str) {
    let vertex = Vertex::new(
        VertexId::try_from_int64(id).expect("test vertex id"),
        graphdb_core::vertex_edge_path::Tag::new(
            "Person".to_string(),
            vec![
                ("id".to_string(), Value::BigInt(id)),
                ("name".to_string(), Value::string(name)),
            ]
            .into_iter()
            .collect(),
        ),
    );
    storage.insert_vertex("test_space", vertex).unwrap();
}

pub(super) fn setup_string_id_space(storage: &mut GraphStorage) {
    let mut space = SpaceInfo::new("str_space".to_string()).with_vid_type(DataType::String);
    storage.create_space(&mut space).unwrap();

    let tag = graphdb_core::types::TagInfo::new("Node".to_string()).with_properties(vec![
        PropertyDef::new("id".to_string(), DataType::String),
        PropertyDef::new("name".to_string(), DataType::String),
    ]);
    storage.create_tag("str_space", &tag).unwrap();

    let edge = EdgeTypeInfo::new("LINK".to_string());
    storage.create_edge_type("str_space", &edge).unwrap();
}

pub(super) fn setup_serial_person_tag(storage: &mut GraphStorage) -> u32 {
    // A SERIAL column can never be the primary key (its values are
    // allocated independently of the vertex id), so the mirror column
    // `vid` leads and the serial `id` follows as a business column.
    let tag = graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
        PropertyDef::new("vid".to_string(), DataType::BigInt),
        PropertyDef::new("id".to_string(), DataType::BigInt).with_serial(true),
        PropertyDef::new("name".to_string(), DataType::String),
    ]);
    storage
        .create_tag("test_space", &tag)
        .expect("Failed to create tag")
}

pub(super) fn insert_serial_vertex(storage: &mut GraphStorage, vid: i64, name: &str) {
    let vertex = Vertex::new(
        VertexId::try_from_int64(vid).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            vec![("name".to_string(), Value::string(name))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("test_space", vertex).unwrap();
}
