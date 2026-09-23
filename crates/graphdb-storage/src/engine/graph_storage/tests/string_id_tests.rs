use super::*;

#[test]
fn test_string_id_get_node_edges_in() {
    let mut storage = create_test_storage();
    setup_string_id_space(&mut storage);

    let v1 = Vertex::new(
        VertexId::try_from_string("a").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("A"))]
                .into_iter()
                .collect(),
        ),
    );
    let v2 = Vertex::new(
        VertexId::try_from_string("b").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("B"))]
                .into_iter()
                .collect(),
        ),
    );
    let v3 = Vertex::new(
        VertexId::try_from_string("c").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("C"))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();
    storage.insert_vertex("str_space", v3).unwrap();

    let edge1 = Edge::new(
        VertexId::try_from_string("b").expect("test vertex id"),
        VertexId::try_from_string("a").expect("test vertex id"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    let edge2 = Edge::new(
        VertexId::try_from_string("c").expect("test vertex id"),
        VertexId::try_from_string("a").expect("test vertex id"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge1).unwrap();
    storage.insert_edge("str_space", edge2).unwrap();

    let in_edges = storage
        .get_node_edges(
            "str_space",
            &VertexId::try_from_string("a").expect("test vertex id"),
            EdgeDirection::In,
        )
        .unwrap();
    assert_eq!(in_edges.len(), 2, "Node 'a' should have 2 incoming edges");

    for edge in &in_edges {
        assert_eq!(
            edge.dst,
            VertexId::try_from_string("a").expect("test vertex id"),
            "dst should be 'a'"
        );
        assert!(
            edge.src == VertexId::try_from_string("b").expect("test vertex id")
                || edge.src == VertexId::try_from_string("c").expect("test vertex id"),
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
        VertexId::try_from_string("a").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("A"))]
                .into_iter()
                .collect(),
        ),
    );
    let v2 = Vertex::new(
        VertexId::try_from_string("b").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("B"))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();

    let edge = Edge::new(
        VertexId::try_from_string("a").expect("test vertex id"),
        VertexId::try_from_string("b").expect("test vertex id"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge).unwrap();

    let out_edges = storage
        .get_node_edges(
            "str_space",
            &VertexId::try_from_string("a").expect("test vertex id"),
            EdgeDirection::Out,
        )
        .unwrap();
    assert_eq!(out_edges.len(), 1, "Node 'a' should have 1 outgoing edge");
    assert_eq!(
        out_edges[0].src,
        VertexId::try_from_string("a").expect("test vertex id")
    );
    assert_eq!(
        out_edges[0].dst,
        VertexId::try_from_string("b").expect("test vertex id")
    );
}

#[test]
fn test_string_id_scan_edges_by_type() {
    let mut storage = create_test_storage();
    setup_string_id_space(&mut storage);

    let v1 = Vertex::new(
        VertexId::try_from_string("x").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("X"))]
                .into_iter()
                .collect(),
        ),
    );
    let v2 = Vertex::new(
        VertexId::try_from_string("y").expect("test vertex id"),
        Tag::new(
            "Node".to_string(),
            vec![("name".to_string(), Value::string("Y"))]
                .into_iter()
                .collect(),
        ),
    );
    storage.insert_vertex("str_space", v1).unwrap();
    storage.insert_vertex("str_space", v2).unwrap();

    let edge = Edge::new(
        VertexId::try_from_string("x").expect("test vertex id"),
        VertexId::try_from_string("y").expect("test vertex id"),
        "LINK".to_string(),
        0,
        std::collections::HashMap::new(),
    );
    storage.insert_edge("str_space", edge).unwrap();

    let edges = storage.scan_edges_by_type("str_space", "LINK").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].src,
        VertexId::try_from_string("x").expect("test vertex id")
    );
    assert_eq!(
        edges[0].dst,
        VertexId::try_from_string("y").expect("test vertex id")
    );
}
