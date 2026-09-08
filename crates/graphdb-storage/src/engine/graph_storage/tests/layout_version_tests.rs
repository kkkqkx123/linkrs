use super::*;

#[test]
fn layout_version_is_monotonic_and_domain_evidence_tracks_numeric_ids() {
    use crate::StorageReader;

    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let initial = storage.layout_version();
    assert!(initial > 0, "layout version starts at a non-zero value");

    // Before any writes the domain is unproven.
    assert!(storage.vertex_id_domain("test_space").is_none());

    // Insert numeric vertex ids: domain evidence appears and covers them.
    for id in [1i64, 10, 100] {
        storage
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Person".to_string(),
                        [("name".to_string(), Value::string(format!("p{id}")))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("insert numeric vertex");
    }
    let domain = storage
        .vertex_id_domain("test_space")
        .expect("numeric-only space must prove its vertex-id domain");
    assert_eq!(domain.start, 1);
    assert_eq!(domain.end, 101);

    // Inserting a vertex with a string id invalidates the domain evidence.
    storage
        .insert_vertex(
            "test_space",
            Vertex::new(
                VertexId::from_string("alice"),
                vec![Tag::new(
                    "Person".to_string(),
                    [("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            ),
        )
        .expect("insert string vertex");
    assert!(
        storage.vertex_id_domain("test_space").is_none(),
        "a string vertex id must invalidate the numeric domain evidence"
    );
}

#[test]
fn layout_version_bumps_after_compaction_and_restore() {
    use crate::StorageReader;

    let (temp_dir, mut storage) = create_persistent_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    for id in 1..=10i64 {
        storage
            .insert_vertex(
                "test_space",
                Vertex::new(
                    VertexId::from_int64(id),
                    vec![Tag::new(
                        "Person".to_string(),
                        [("name".to_string(), Value::string(format!("p{id}")))]
                            .into_iter()
                            .collect(),
                    )],
                ),
            )
            .expect("insert vertex");
    }

    let before_compact = storage.layout_version();

    // Run a compact maintenance pass via the public admin API.
    storage
        .compact(&graphdb_core::types::CompactConfig {
            enable_structure_compaction: true,
            ..Default::default()
        })
        .expect("compact should succeed");

    let after_compact = storage.layout_version();
    assert!(
        after_compact > before_compact,
        "compaction must bump the layout version ({before_compact} -> {after_compact})"
    );

    // The domain evidence survives a save/reload round-trip.
    storage.save_to_disk().expect("save to disk");
    let domain_before = storage.vertex_id_domain("test_space");

    drop(storage);
    let mut reloaded =
        GraphStorage::new_with_path(temp_dir.path().to_path_buf()).expect("reload storage");
    reloaded.load_from_disk().expect("load from disk");

    let domain_after = reloaded.vertex_id_domain("test_space");
    assert_eq!(
        domain_after, domain_before,
        "vertex-id domain evidence must be rebuilt after restore"
    );
    assert!(domain_after.is_some());
    assert!(
        reloaded.layout_version() > 1,
        "restore must bump the fresh instance's layout version"
    );
}
