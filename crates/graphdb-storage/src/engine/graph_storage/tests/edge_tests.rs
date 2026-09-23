use super::*;

#[test]
fn test_edge_property_index_range_lookup() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let edge_type =
        graphdb_core::types::EdgeTypeInfo::new("WEIGHTED".to_string()).with_properties(vec![
            PropertyDef::new("weight".to_string(), DataType::BigInt),
        ]);
    storage
        .create_edge_type("test_space", &edge_type)
        .expect("Failed to create edge type");

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_test_vertex(&mut storage, 3, "Carol");

    let make_edge = |src: i64, dst: i64, weight: i64| {
        Edge::new(
            VertexId::try_from_int64(src).expect("test vertex id"),
            VertexId::try_from_int64(dst).expect("test vertex id"),
            "WEIGHTED".to_string(),
            0,
            [("weight".to_string(), Value::BigInt(weight))]
                .into_iter()
                .collect(),
        )
    };
    storage
        .insert_edge("test_space", make_edge(1, 2, 10))
        .unwrap();
    storage
        .insert_edge("test_space", make_edge(1, 3, 25))
        .unwrap();
    storage
        .insert_edge("test_space", make_edge(2, 3, 5))
        .unwrap();

    // Enable after inserts: build path indexes existing edges.
    assert!(!storage
        .has_edge_property_index("test_space", "WEIGHTED")
        .unwrap());
    storage
        .enable_edge_property_index("test_space", "WEIGHTED", 64 * 1024 * 1024)
        .unwrap();
    assert!(storage
        .has_edge_property_index("test_space", "WEIGHTED")
        .unwrap());

    // weight >= 20
    let edges = storage
        .lookup_edges_by_property_range(
            "test_space",
            "WEIGHTED",
            "weight",
            Some(&Value::BigInt(20)),
            None,
            true,
            false,
        )
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].src,
        VertexId::try_from_int64(1).expect("test vertex id")
    );
    assert_eq!(
        edges[0].dst,
        VertexId::try_from_int64(3).expect("test vertex id")
    );
    assert_eq!(edges[0].props.get("weight"), Some(&Value::BigInt(25)));

    // 5 <= weight < 25
    let edges = storage
        .lookup_edges_by_property_range(
            "test_space",
            "WEIGHTED",
            "weight",
            Some(&Value::BigInt(5)),
            Some(&Value::BigInt(25)),
            true,
            false,
        )
        .unwrap();
    assert_eq!(edges.len(), 2);

    // Disable frees the index.
    storage
        .disable_edge_property_index("test_space", "WEIGHTED")
        .unwrap();
    assert!(!storage
        .has_edge_property_index("test_space", "WEIGHTED")
        .unwrap());
    let edges = storage
        .lookup_edges_by_property_range(
            "test_space",
            "WEIGHTED",
            "weight",
            Some(&Value::BigInt(20)),
            None,
            true,
            false,
        )
        .unwrap();
    assert!(edges.is_empty());
}

#[test]
fn test_insert_and_get_edge() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "KNOWS".to_string(),
        0,
        vec![("since".to_string(), Value::Int(2020))]
            .into_iter()
            .collect(),
    );
    storage.insert_edge("test_space", edge).unwrap();

    let retrieved = storage
        .get_edge(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            &VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS",
            0,
        )
        .unwrap();
    assert!(retrieved.is_some());
    assert_eq!(
        retrieved.as_ref().unwrap().src,
        VertexId::try_from_int64(1).expect("test vertex id")
    );
    assert_eq!(
        retrieved.as_ref().unwrap().dst,
        VertexId::try_from_int64(2).expect("test vertex id")
    );
}

#[test]
fn test_delete_edge() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "KNOWS".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("test_space", edge).unwrap();

    storage
        .delete_edge(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            &VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS",
            0,
        )
        .unwrap();

    let retrieved = storage
        .get_edge(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            &VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS",
            0,
        )
        .unwrap();
    assert!(retrieved.is_none());
}

#[test]
fn test_get_node_edges() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");
    insert_test_vertex(&mut storage, 3, "Charlie");

    for dst in &[2i64, 3] {
        let edge = Edge::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            VertexId::try_from_int64(*dst).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();
    }

    let out_edges = storage
        .get_node_edges(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            EdgeDirection::Out,
        )
        .unwrap();
    assert_eq!(out_edges.len(), 2);

    let in_edges = storage
        .get_node_edges(
            "test_space",
            &VertexId::try_from_int64(2).expect("test vertex id"),
            EdgeDirection::In,
        )
        .unwrap();
    assert_eq!(in_edges.len(), 1);
}

/// The batched accessors must agree with
/// `get_node_edges` for out/in/both directions and edge-type filtering.
#[test]
fn test_batch_accessors_match_get_node_edges() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    for i in 1..=5i64 {
        insert_test_vertex(&mut storage, i, &format!("v{i}"));
    }
    for (src, dst) in [(1i64, 2), (1, 3), (2, 3), (3, 1), (4, 1)] {
        let edge = Edge::new(
            VertexId::try_from_int64(src).expect("test vertex id"),
            VertexId::try_from_int64(dst).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        );
        storage.insert_edge("test_space", edge).unwrap();
    }

    let seeds = [
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        VertexId::try_from_int64(5).expect("test vertex id"),
    ];
    let knowses = vec!["KNOWS".to_string()];

    for direction in [EdgeDirection::Out, EdgeDirection::In, EdgeDirection::Both] {
        for (edge_types, label) in [(Vec::<String>::new(), "all"), (knowses.clone(), "KNOWS")] {
            // Reference: distinct dst ids from get_node_edges.
            let mut expected: Vec<Vec<VertexId>> = Vec::new();
            for seed in &seeds {
                let edges = storage
                    .get_node_edges("test_space", seed, direction)
                    .unwrap();
                let mut dsts: Vec<VertexId> = edges
                    .iter()
                    .filter(|e| edge_types.is_empty() || edge_types.contains(&e.edge_type))
                    .map(|e| {
                        if matches!(direction, EdgeDirection::Out) {
                            e.dst
                        } else if matches!(direction, EdgeDirection::In) {
                            e.src
                        } else if e.src == *seed {
                            e.dst
                        } else {
                            e.src
                        }
                    })
                    .collect();
                dsts.sort();
                dsts.dedup();
                expected.push(dsts);
            }

            let batch = storage
                .neighbor_dst_ids_batch("test_space", &seeds, direction, &edge_types)
                .unwrap();
            let actual: Vec<Vec<VertexId>> = batch
                .into_iter()
                .map(|mut dsts| {
                    dsts.sort();
                    dsts.dedup();
                    dsts
                })
                .collect();
            assert_eq!(
                expected, actual,
                "neighbor batch mismatch ({label}, {direction:?})"
            );

            let degrees: Vec<usize> = storage
                .out_degree_batch("test_space", &seeds, direction, &edge_types)
                .unwrap();
            let expected_degrees: Vec<usize> = seeds
                .iter()
                .map(|seed| {
                    storage
                        .get_node_edges("test_space", seed, direction)
                        .unwrap()
                        .iter()
                        .filter(|edge| {
                            edge_types.is_empty() || edge_types.contains(&edge.edge_type)
                        })
                        .count()
                })
                .collect();
            assert_eq!(
                expected_degrees, degrees,
                "degree batch mismatch ({label}, {direction:?})"
            );
        }
    }
}

#[test]
fn test_scan_edges_by_type() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "KNOWS".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("test_space", edge).unwrap();

    let edges = storage.scan_edges_by_type("test_space", "KNOWS").unwrap();
    assert_eq!(edges.len(), 1);
}

#[test]
fn test_batch_insert_edges_rolls_back_on_failure() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);

    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edges = vec![
        Edge::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        ),
        Edge::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            VertexId::try_from_int64(3).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        ),
    ];

    assert!(storage.batch_insert_edges("test_space", edges).is_err());
    assert_eq!(
        storage
            .scan_edges_by_type("test_space", "KNOWS")
            .unwrap()
            .len(),
        0
    );
}

fn batch_equivalence_edges() -> Vec<Edge> {
    let mut edges = Vec::new();
    for src in 1..=5i64 {
        for k in 0..3i64 {
            let mut props = std::collections::HashMap::new();
            props.insert(
                "since".to_string(),
                graphdb_core::Value::from((src * 10 + k) as i32),
            );
            edges.push(Edge::new(
                VertexId::try_from_int64(src).expect("test vertex id"),
                VertexId::try_from_int64(src + k + 1).expect("test vertex id"),
                "KNOWS".to_string(),
                k,
                props,
            ));
        }
    }
    edges
}

fn sorted_edge_keys(edges: Vec<graphdb_core::Edge>) -> Vec<(i64, i64, i64, String)> {
    let mut keys: Vec<(i64, i64, i64, String)> = edges
        .into_iter()
        .map(|edge| {
            (
                edge.src.as_int64().unwrap_or(-1),
                edge.dst.as_int64().unwrap_or(-1),
                edge.ranking,
                format!("{:?}", edge.props.get("since")),
            )
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn test_batch_insert_edges_matches_sequential() {
    let edges = batch_equivalence_edges();

    let mut reference = create_test_storage();
    setup_space(&mut reference);
    setup_person_tag(&mut reference);
    setup_knows_edge(&mut reference);
    for id in 1..=9i64 {
        insert_test_vertex(&mut reference, id, &format!("person{id}"));
    }
    for edge in edges.clone() {
        reference.insert_edge("test_space", edge).unwrap();
    }

    let mut batched = create_test_storage();
    setup_space(&mut batched);
    setup_person_tag(&mut batched);
    setup_knows_edge(&mut batched);
    for id in 1..=9i64 {
        insert_test_vertex(&mut batched, id, &format!("person{id}"));
    }
    batched.batch_insert_edges("test_space", edges).unwrap();

    let reference_keys =
        sorted_edge_keys(reference.scan_edges_by_type("test_space", "KNOWS").unwrap());
    let batched_keys = sorted_edge_keys(batched.scan_edges_by_type("test_space", "KNOWS").unwrap());
    assert_eq!(batched_keys, reference_keys);
    assert_eq!(batched_keys.len(), 15);
}

#[test]
fn test_batch_insert_edges_rejects_intra_batch_duplicates() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);
    insert_test_vertex(&mut storage, 1, "Alice");
    insert_test_vertex(&mut storage, 2, "Bob");

    let edges = vec![
        Edge::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        ),
        Edge::new(
            VertexId::try_from_int64(1).expect("test vertex id"),
            VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        ),
    ];
    assert!(storage.batch_insert_edges("test_space", edges).is_err());
    assert_eq!(
        storage
            .scan_edges_by_type("test_space", "KNOWS")
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn test_get_edge_projected() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    let edge_type = graphdb_core::types::EdgeTypeInfo::new("KNOWS".to_string())
        .with_src_tag("Person".to_string())
        .with_dst_tag("Person".to_string())
        .with_properties(vec![
            PropertyDef::new("since".to_string(), DataType::Int),
            PropertyDef::new("weight".to_string(), DataType::Int),
        ]);
    storage.create_edge_type("test_space", &edge_type).unwrap();

    let src = VertexId::try_from_int64(1).expect("test vertex id");
    let dst = VertexId::try_from_int64(2).expect("test vertex id");
    storage
        .insert_vertex(
            "test_space",
            Vertex::new(
                src,
                Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .unwrap();
    storage
        .insert_vertex(
            "test_space",
            Vertex::new(
                dst,
                Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .unwrap();
    storage
        .insert_edge(
            "test_space",
            Edge::new(
                src,
                dst,
                "KNOWS".to_string(),
                0,
                vec![
                    ("since".to_string(), Value::BigInt(2020)),
                    ("weight".to_string(), Value::BigInt(7)),
                ]
                .into_iter()
                .collect(),
            ),
        )
        .unwrap();

    let full = storage
        .get_edge("test_space", &src, &dst, "KNOWS", 0)
        .unwrap()
        .expect("edge exists");
    assert_eq!(full.properties().len(), 2);

    let projected = storage
        .get_edge_projected(
            "test_space",
            &src,
            &dst,
            "KNOWS",
            0,
            &["weight".to_string()],
        )
        .unwrap()
        .expect("edge exists");
    assert_eq!(projected.properties().len(), 1);
    assert_eq!(
        projected.properties().get("weight"),
        Some(&Value::BigInt(7))
    );
    assert!(!projected.properties().contains_key("since"));

    // Full read must not be poisoned by the projected read.
    let full_again = storage
        .get_edge("test_space", &src, &dst, "KNOWS", 0)
        .unwrap()
        .expect("edge exists");
    assert_eq!(full_again.properties().len(), 2);
}

#[test]
fn test_batch_delete_edges_removes_many_keys_under_one_timestamp() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    setup_knows_edge(&mut storage);
    for (id, name) in [(1, "Alice"), (2, "Bob"), (3, "Carol"), (4, "Dave")] {
        insert_test_vertex(&mut storage, id, name);
    }
    let edge = |src: i64, dst: i64| {
        Edge::new(
            VertexId::try_from_int64(src).expect("test vertex id"),
            VertexId::try_from_int64(dst).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
            std::collections::HashMap::new(),
        )
    };
    storage
        .batch_insert_edges("test_space", vec![edge(1, 2), edge(1, 3), edge(3, 4)])
        .unwrap();

    let key = |src: i64, dst: i64| {
        graphdb_core::EdgeDeleteKey::new(
            VertexId::try_from_int64(src).expect("test vertex id"),
            VertexId::try_from_int64(dst).expect("test vertex id"),
            "KNOWS".to_string(),
            0,
        )
    };
    // Two live keys plus one missing key: the missing key is a no-op and the
    // reported count covers the applied deletes only.
    let deleted = storage
        .batch_delete_edges("test_space", &[key(1, 2), key(1, 3), key(2, 4)])
        .unwrap();
    assert_eq!(deleted, 2);
    let remaining = storage.scan_edges_by_type("test_space", "KNOWS").unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].src,
        VertexId::try_from_int64(3).expect("test vertex id")
    );
    assert!(storage
        .get_edge(
            "test_space",
            &VertexId::try_from_int64(1).expect("test vertex id"),
            &VertexId::try_from_int64(2).expect("test vertex id"),
            "KNOWS",
            0
        )
        .unwrap()
        .is_none());

    // Re-deleting tombstoned keys fails like the single delete, with the
    // surviving edge untouched.
    assert!(storage
        .batch_delete_edges("test_space", &[key(1, 2), key(1, 3), key(2, 4)])
        .is_err());
    assert_eq!(
        storage
            .scan_edges_by_type("test_space", "KNOWS")
            .unwrap()
            .len(),
        1
    );
}
