use super::*;

#[test]
fn test_string_id_get_node_edges_in() {
    let mut storage = create_test_storage();
    setup_string_id_space(&mut storage);

    let v1 = Vertex::new(
        VertexId::from_string("a"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("A"))]
                .into_iter()
                .collect(),
        )],
    );
    let v2 = Vertex::new(
        VertexId::from_string("b"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("B"))]
                .into_iter()
                .collect(),
        )],
    );
    let v3 = Vertex::new(
        VertexId::from_string("c"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("C"))]
                .into_iter()
                .collect(),
        )],
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();
    storage.insert_vertex("str_space", v3).unwrap();

    let edge1 = Edge::new(
        VertexId::from_string("b"),
        VertexId::from_string("a"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    let edge2 = Edge::new(
        VertexId::from_string("c"),
        VertexId::from_string("a"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge1).unwrap();
    storage.insert_edge("str_space", edge2).unwrap();

    let in_edges = storage
        .get_node_edges("str_space", &VertexId::from_string("a"), EdgeDirection::In)
        .unwrap();
    assert_eq!(in_edges.len(), 2, "Node 'a' should have 2 incoming edges");

    for edge in &in_edges {
        assert_eq!(edge.dst, VertexId::from_string("a"), "dst should be 'a'");
        assert!(
            edge.src == VertexId::from_string("b") || edge.src == VertexId::from_string("c"),
            "src should be 'b' or 'c', got {:?}",
            edge.src
        );
    }
}

#[test]
fn test_string_id_get_node_edges_out() {
    let mut storage = create_test_storage();
    setup_string_id_space(&mut storage);

    let v1 = Vertex::new(
        VertexId::from_string("a"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("A"))]
                .into_iter()
                .collect(),
        )],
    );
    let v2 = Vertex::new(
        VertexId::from_string("b"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("B"))]
                .into_iter()
                .collect(),
        )],
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();

    let edge = Edge::new(
        VertexId::from_string("a"),
        VertexId::from_string("b"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge).unwrap();

    let out_edges = storage
        .get_node_edges("str_space", &VertexId::from_string("a"), EdgeDirection::Out)
        .unwrap();
    assert_eq!(out_edges.len(), 1, "Node 'a' should have 1 outgoing edge");
    assert_eq!(out_edges[0].src, VertexId::from_string("a"));
    assert_eq!(out_edges[0].dst, VertexId::from_string("b"));
}

#[test]
fn test_string_id_scan_edges_by_type() {
    let mut storage = create_test_storage();
    setup_string_id_space(&mut storage);

    let v1 = Vertex::new(
        VertexId::from_string("x"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("X"))]
                .into_iter()
                .collect(),
        )],
    );
    let v2 = Vertex::new(
        VertexId::from_string("y"),
        vec![Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("Y"))]
                .into_iter()
                .collect(),
        )],
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();

    let edge = Edge::new(
        VertexId::from_string("x"),
        VertexId::from_string("y"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge).unwrap();

    let edges = storage.scan_edges_by_type("str_space", "LINK").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].src, VertexId::from_string("x"));
    assert_eq!(edges[0].dst, VertexId::from_string("y"));
}
