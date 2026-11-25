use super::*;

use crate::StorageReader;

fn insert_vertex(storage: &mut GraphStorage, tag: &str, id: i64) {
    let vertex = Vertex::new(
        VertexId::try_from_int64(id).expect("test vertex id"),
        linkrs_core::vertex_edge_path::Tag::new(
            tag.to_string(),
            [("id".into(), Value::BigInt(id))].into_iter().collect(),
        ),
    );
    storage
        .insert_vertex("test_space", vertex)
        .expect("insert vertex");
}

#[test]
fn incoming_exists_uses_reverse_csr_and_keeps_input_order() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);
    storage
        .create_tag(
            "test_space",
            &linkrs_core::types::TagInfo::new("Comment_".to_string())
                .with_properties(vec![PropertyDef::new("id".into(), DataType::BigInt)]),
        )
        .expect("create comment tag");
    storage
        .create_edge_type(
            "test_space",
            &EdgeTypeInfo::new("HASCREATOR".to_string())
                .with_src_tag("Comment_".to_string())
                .with_dst_tag("Person".to_string()),
        )
        .expect("create edge type");

    for id in [1, 2, 3, 10, 11] {
        let tag = if id < 10 { "Comment_" } else { "Person" };
        insert_vertex(&mut storage, tag, id);
    }
    storage
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::try_from_int64(1).expect("src"),
                VertexId::try_from_int64(10).expect("dst"),
                "HASCREATOR".to_string(),
                0,
                Default::default(),
            ),
        )
        .expect("insert reverse edge");
    // A second parallel edge only changes multiplicity, not existence.
    storage
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::try_from_int64(2).expect("src"),
                VertexId::try_from_int64(11).expect("dst"),
                "HASCREATOR".to_string(),
                0,
                Default::default(),
            ),
        )
        .expect("insert first edge");
    storage
        .insert_edge(
            "test_space",
            Edge::new(
                VertexId::try_from_int64(3).expect("src"),
                VertexId::try_from_int64(11).expect("dst"),
                "HASCREATOR".to_string(),
                0,
                Default::default(),
            ),
        )
        .expect("insert parallel edge");

    let flags = storage
        .incoming_edge_exists_batch(
            "test_space",
            &[
                VertexId::try_from_int64(11).expect("id"),
                VertexId::try_from_int64(10).expect("id"),
                VertexId::try_from_int64(2).expect("id"),
            ],
            "Comment_",
            "Person",
            "HASCREATOR",
        )
        .expect("existence read should succeed");
    assert_eq!(flags, vec![true, true, false]);
}
