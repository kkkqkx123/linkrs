//! Dangling-free proof token lifecycle.
//!
//! Inserts validate endpoints, so only plain vertex deletes (which keep
//! incident edges) can introduce dangling endpoints. The token starts
//! proven for freshly created spaces, is revoked for incident edge types
//! on plain deletes, and is re-established by a fully successful repair.

use super::{
    create_test_storage, insert_test_vertex, setup_knows_edge, setup_person_tag, setup_space,
};
use crate::{StorageReader, StorageSchemaOps, StorageWriter};
use linkrs_core::types::VertexId;
use linkrs_core::Edge;

fn setup_bound_knows_edge(storage: &mut crate::GraphStorage) {
    let edge = linkrs_core::types::EdgeTypeInfo::new("BOUND_KNOWS".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Person".to_string());
    storage.create_edge_type("test_space", &edge).unwrap();
}

fn insert_bound_knows(storage: &mut crate::GraphStorage, src: i64, dst: i64) {
    let edge = Edge::new(
        VertexId::try_from_int64(src).expect("test vertex id"),
        VertexId::try_from_int64(dst).expect("test vertex id"),
        "BOUND_KNOWS".into(),
        0,
        Default::default(),
    );
    storage.insert_edge("test_space", edge).unwrap();
}

fn insert_knows(storage: &mut crate::GraphStorage, src: i64, dst: i64) {
    let edge = Edge::new(
        VertexId::try_from_int64(src).expect("test vertex id"),
        VertexId::try_from_int64(dst).expect("test vertex id"),
        "KNOWS".into(),
        0,
        Default::default(),
    );
    storage.insert_edge("test_space", edge).unwrap();
}

fn vid(id: i64) -> VertexId {
    VertexId::try_from_int64(id).expect("test vertex id")
}

#[test]
fn fresh_space_starts_proven_and_survives_clean_writes() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);
    assert!(storage.edge_type_dangling_free("test_space", "KNOWS"));

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_knows(&mut storage, 1, 2);
    assert!(
        storage.edge_type_dangling_free("test_space", "KNOWS"),
        "validated inserts must preserve the proof"
    );
}

#[test]
fn plain_delete_revokes_only_incident_types() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_test_vertex(&mut storage, 3, "Carol");
    insert_knows(&mut storage, 1, 2);

    // Deleting an isolated vertex touches no edge: proof survives.
    storage
        .delete_vertex("test_space", "Person", &vid(3))
        .unwrap();
    assert!(storage.edge_type_dangling_free("test_space", "KNOWS"));

    // Deleting an endpoint leaves the edge behind: proof revoked.
    storage
        .delete_vertex("test_space", "Person", &vid(1))
        .unwrap();
    assert!(!storage.edge_type_dangling_free("test_space", "KNOWS"));
}

#[test]
fn repair_reestablishes_proof() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_bound_knows_edge(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_bound_knows(&mut storage, 1, 2);
    storage
        .delete_vertex("test_space", "Person", &vid(1))
        .unwrap();
    assert!(!storage.edge_type_dangling_free("test_space", "BOUND_KNOWS"));

    let repaired = super::super::ops::repair_dangling_edges(&storage.ctx, "test_space")
        .expect("repair dangling edges");
    assert_eq!(repaired, 1);
    assert!(
        storage.edge_type_dangling_free("test_space", "BOUND_KNOWS"),
        "successful repair re-establishes the proof"
    );
}

#[test]
fn cascade_delete_preserves_proof() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_knows(&mut storage, 1, 2);

    storage
        .delete_vertex_with_edges("test_space", "Person", &vid(1))
        .unwrap();
    assert!(
        storage.edge_type_dangling_free("test_space", "KNOWS"),
        "cascade removes incident edges first"
    );
}
