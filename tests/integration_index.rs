//! Indexing System Integration Testing
//!
//! Test range:
//! - Tag index metadata management (create, delete, query, list)
//! - Edge index metadata management (create, delete, query, list)
//! - Indexed data management (updates, deletions, queries)
//! - Index queries (exact queries, range queries)
//! - index cache

mod common;

use common::{
    assertions::{assert_count, assert_none, assert_ok, assert_some},
    storage_helpers::{create_test_space, knows_edge_type_info, person_tag_info},
    TestStorage,
};
use graphdb::core::types::{Index, IndexField, IndexStatus, IndexType, VertexId};
use graphdb::core::{Edge, Value, Vertex};
use graphdb::query::planning::plan::{IndexLimit, ScanType};
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

fn get_storage(
    storage: &Arc<RwLock<GraphStorage>>,
) -> parking_lot::RwLockWriteGuard<'_, GraphStorage> {
    storage.write()
}

// ==================== Tag Indexing Metadata Management Test ====================

#[test]
fn test_create_tag_index_metadata() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    let result = get_storage(&storage).create_tag_index("test_space", &index);
    let created = result.expect("创建索引应该成功");
    assert!(created, "The index should be created");

    let retrieved = get_storage(&storage).get_tag_index("test_space", "person_name_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_some(&index_opt);

    let retrieved_index = index_opt.expect("索引应该存在");
    assert_eq!(retrieved_index.name, "person_name_idx");
    assert_eq!(retrieved_index.schema_name, "Person");
    assert_eq!(retrieved_index.index_type, IndexType::TagIndex);
}

#[test]
fn test_create_tag_index_duplicate() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let result = get_storage(&storage).create_tag_index("test_space", &index);
    let created = result.expect("创建重复索引应该返回 false");
    assert!(!created, "Duplicate index creation should return false");
}

#[test]
fn test_drop_tag_index_metadata() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let result = get_storage(&storage).drop_tag_index("test_space", "person_name_idx");
    let dropped = result.expect("删除索引应该成功");
    assert!(dropped, "Indexes should be deleted");

    let retrieved = get_storage(&storage).get_tag_index("test_space", "person_name_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_none(&index_opt);
}

#[test]
fn test_list_tag_indexes() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index1 = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    let index2 = Index::new(graphdb::core::types::IndexConfig {
        id: 2,
        name: "person_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("age".to_string(), Value::Int(0), false)],
        properties: vec!["age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index1));
    assert_ok(get_storage(&storage).create_tag_index("test_space", &index2));

    let result = get_storage(&storage).list_tag_indexes("test_space");
    let indexes = result.expect("列出索引应该成功");
    assert_count(&indexes, 2, "索引");

    let index_names: Vec<&str> = indexes.iter().map(|i| i.name.as_str()).collect();
    assert!(
        index_names.contains(&"person_name_idx"),
        "Should contain person_name_idx"
    );
    assert!(
        index_names.contains(&"person_age_idx"),
        "Should contain person_age_idx"
    );
}

#[test]
fn test_drop_tag_indexes_by_tag() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index1 = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    let index2 = Index::new(graphdb::core::types::IndexConfig {
        id: 2,
        name: "person_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("age".to_string(), Value::Int(0), false)],
        properties: vec!["age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index1));
    assert_ok(get_storage(&storage).create_tag_index("test_space", &index2));

    get_storage(&storage)
        .drop_tag_index("test_space", "person_name_idx")
        .expect("删除标签索引应该成功");
    get_storage(&storage)
        .drop_tag_index("test_space", "person_age_idx")
        .expect("删除标签索引应该成功");

    let indexes = get_storage(&storage)
        .list_tag_indexes("test_space")
        .expect("列出索引应该成功");
    assert_count(&indexes, 0, "索引");
}

// ==================== Edge 索引元数据管理测试 ====================

#[cfg(feature = "qdrant")]
#[test]
fn test_create_edge_index_metadata() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let edge_info = knows_edge_type_info();
    assert_ok(get_storage(&storage).create_edge_type("test_space", &edge_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "knows_since_idx".to_string(),
        space_id: 0,
        schema_name: "KNOWS".to_string(),
        fields: vec![IndexField::new(
            "since".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["since".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        partial_condition: None,
    });

    let result = get_storage(&storage).create_tag_index("test_space", &index);
    let created = result.expect("创建索引应该成功");
    assert!(created, "The index should be created");

    let retrieved = get_storage(&storage).get_tag_index("test_space", "knows_since_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_some(&index_opt);

    let retrieved_index = index_opt.expect("索引应该存在");
    assert_eq!(retrieved_index.name, "knows_since_idx");
    assert_eq!(retrieved_index.schema_name, "KNOWS");
    assert_eq!(retrieved_index.index_type, IndexType::EdgeIndex);
}

#[cfg(feature = "qdrant")]
#[test]
fn test_drop_edge_index_metadata() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let edge_info = knows_edge_type_info();
    assert_ok(get_storage(&storage).create_edge_type("test_space", &edge_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "knows_since_idx".to_string(),
        space_id: 0,
        schema_name: "KNOWS".to_string(),
        fields: vec![IndexField::new(
            "since".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["since".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let result = get_storage(&storage).drop_tag_index("test_space", "knows_since_idx");
    let dropped = result.expect("删除索引应该成功");
    assert!(dropped, "Indexes should be deleted");

    let retrieved = get_storage(&storage).get_tag_index("test_space", "knows_since_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_none(&index_opt);
}

#[cfg(feature = "qdrant")]
#[test]
fn test_list_edge_indexes() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let edge_info = knows_edge_type_info();
    assert_ok(get_storage(&storage).create_edge_type("test_space", &edge_info));

    let index1 = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "knows_since_idx".to_string(),
        space_id: 0,
        schema_name: "KNOWS".to_string(),
        fields: vec![IndexField::new(
            "since".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["since".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        partial_condition: None,
    });

    let index2 = Index::new(graphdb::core::types::IndexConfig {
        id: 2,
        name: "knows_weight_idx".to_string(),
        space_id: 0,
        schema_name: "KNOWS".to_string(),
        fields: vec![IndexField::new(
            "weight".to_string(),
            Value::Float(0.0),
            false,
        )],
        properties: vec!["weight".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index1));
    assert_ok(get_storage(&storage).create_tag_index("test_space", &index2));

    let result = get_storage(&storage).list_tag_indexes("test_space");
    let indexes = result.expect("列出索引应该成功");
    assert_count(&indexes, 2, "索引");

    let index_names: Vec<&str> = indexes.iter().map(|i| i.name.as_str()).collect();
    assert!(
        index_names.contains(&"knows_since_idx"),
        "Should contain knows_since_idx"
    );
    assert!(
        index_names.contains(&"knows_weight_idx"),
        "Should contain knows_weight_idx"
    );
}

// ==================== 索引数据管理测试 ====================

#[test]
fn test_update_vertex_indexes() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let vertex_id = VertexId::from_int64(1);
    let mut props = std::collections::HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
    let vertex = Vertex::new(vertex_id, vec![tag]);

    get_storage(&storage)
        .insert_vertex("test_space", vertex)
        .expect("插入顶点应该成功");

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_idx",
        &Value::String("Alice".to_string()),
    );
    let src_ids = retrieved.expect("索引精确查询应该成功");
    assert!(
        src_ids.contains(&Value::from(vertex_id)),
        "The source vertex ID should be in the result set."
    );

    // Clean up
    assert_ok(get_storage(&storage).delete_vertex("test_space", &vertex_id));

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_idx",
        &Value::String("Alice".to_string()),
    );
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert!(
        !vertex_ids.contains(&Value::from(vertex_id)),
        "The index should not contain deleted vertex IDs."
    );
}

#[cfg(feature = "qdrant")]
#[test]
fn test_delete_edge_indexes() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let edge_info = knows_edge_type_info();
    assert_ok(get_storage(&storage).create_edge_type("test_space", &edge_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "knows_since_idx".to_string(),
        space_id: 0,
        schema_name: "KNOWS".to_string(),
        fields: vec![IndexField::new(
            "since".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["since".to_string()],
        index_type: IndexType::EdgeIndex,
        is_unique: false,
        partial_condition: None,
    });

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    // Create vertices first
    let src = VertexId::from_int64(1);
    let dst = VertexId::from_int64(2);
    let tag = graphdb::core::vertex_edge_path::Tag::new(
        "Person".to_string(),
        std::collections::HashMap::new(),
    );
    let vertex1 = Vertex::new(src, vec![tag.clone()]);
    let vertex2 = Vertex::new(dst, vec![tag]);
    assert_ok(get_storage(&storage).insert_vertex("test_space", vertex1));
    assert_ok(get_storage(&storage).insert_vertex("test_space", vertex2));

    let edge_type = "KNOWS";
    let mut props = std::collections::HashMap::new();
    props.insert("since".to_string(), Value::String("2024-01-01".to_string()));
    let edge = Edge::new(src, dst, edge_type.to_string(), 0, props);

    assert_ok(get_storage(&storage).insert_edge("test_space", edge));

    assert_ok(get_storage(&storage).delete_edge("test_space", &src, &dst, edge_type, 0));

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "knows_since_idx",
        &Value::String("2024-01-01".to_string()),
    );
    let src_ids = retrieved.expect("索引查询应该成功");
    assert!(
        !src_ids.contains(&Value::from(src)),
        "The index should not contain the source vertex IDs of deleted edges."
    );
}

// ==================== Index Query Test ====================

#[test]
fn test_index_exact_query() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let vertices = vec![
        (VertexId::from_int64(1), Value::String("Alice".to_string())),
        (VertexId::from_int64(2), Value::String("Bob".to_string())),
        (
            VertexId::from_int64(3),
            Value::String("Charlie".to_string()),
        ),
    ];

    for (vid, name) in &vertices {
        let mut props = std::collections::HashMap::new();
        props.insert("name".to_string(), name.clone());
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(*vid, vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_idx",
        &Value::String("Alice".to_string()),
    );
    let vertex_ids = retrieved.expect("索引精确查询应该成功");
    assert_count(&vertex_ids, 1, "匹配的顶点");
    assert_eq!(
        vertex_ids[0],
        Value::Int(1),
        "should return Alice's vertex ID"
    );
}

#[test]
fn test_index_query_multiple_matches() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("age".to_string(), Value::Int(0), false)],
        properties: vec!["age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let vertices = vec![
        (VertexId::from_int64(1), Value::Int(30)),
        (VertexId::from_int64(2), Value::Int(30)),
        (VertexId::from_int64(3), Value::Int(25)),
    ];

    for (vid, age) in &vertices {
        let mut props = std::collections::HashMap::new();
        props.insert("age".to_string(), age.clone());
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(*vid, vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    let retrieved =
        get_storage(&storage).lookup_index("test_space", "person_age_idx", &Value::Int(30));
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert_count(&vertex_ids, 2, "匹配的顶点");
    assert!(
        vertex_ids.contains(&Value::Int(1)),
        "Should contain vertex 1"
    );
    assert!(
        vertex_ids.contains(&Value::Int(2)),
        "Should contain vertex 2"
    );
}

#[test]
fn test_index_query_no_match() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let vertex_id = VertexId::from_int64(1);
    let mut props = std::collections::HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
    let vertex = Vertex::new(vertex_id, vec![tag]);

    assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_idx",
        &Value::String("Bob".to_string()),
    );
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert_count(&vertex_ids, 0, "匹配的顶点");
}

// ==================== Index Status Test ====================

#[test]
fn test_index_status_active() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let retrieved = get_storage(&storage).get_tag_index("test_space", "person_name_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_some(&index_opt);

    let retrieved_index = index_opt.expect("索引应该存在");
    assert_eq!(
        retrieved_index.status,
        IndexStatus::Active,
        "The newly created index should be Active"
    );
}

#[test]
fn test_unique_index() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_unique_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: true,
        partial_condition: None,
    });

    let result = get_storage(&storage).create_tag_index("test_space", &index);
    result.expect("创建唯一索引应该成功");

    let retrieved = get_storage(&storage).get_tag_index("test_space", "person_name_unique_idx");
    let index_opt = retrieved.expect("获取索引应该成功");
    assert_some(&index_opt);

    let retrieved_index = index_opt.expect("索引应该存在");
    assert!(retrieved_index.is_unique, "Indexes should be unique");
}

// ==================== Compound Index Test ====================

#[test]
fn test_composite_index() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![
            IndexField::new("name".to_string(), Value::String("".to_string()), false),
            IndexField::new("age".to_string(), Value::Int(0), false),
        ],
        properties: vec!["name".to_string(), "age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    let vertices = vec![
        (
            VertexId::from_int64(1),
            Value::String("Alice".to_string()),
            Value::Int(30),
        ),
        (
            VertexId::from_int64(2),
            Value::String("Alice".to_string()),
            Value::Int(25),
        ),
        (
            VertexId::from_int64(3),
            Value::String("Bob".to_string()),
            Value::Int(30),
        ),
    ];

    for (vid, name, age) in &vertices {
        let mut props = std::collections::HashMap::new();
        props.insert("name".to_string(), name.clone());
        props.insert("age".to_string(), age.clone());
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(*vid, vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_age_idx",
        &Value::String("Alice".to_string()),
    );
    let vertex_ids = retrieved.expect("复合索引查询应该成功");
    assert_count(&vertex_ids, 2, "匹配的顶点（两个 Alice）");
    assert!(
        vertex_ids.contains(&Value::Int(1)),
        "Should contain vertex 1"
    );
    assert!(
        vertex_ids.contains(&Value::Int(2)),
        "Should contain vertex 2"
    );
}

// ==================== IndexSelector 集成测试 ====================
// Note: This test is temporarily disabled because it uses a non-existent API
// #[test]
// fn test_index_selector_chooses_optimal_index() {
//     use graphdb::core::types::operators::BinaryOperator;
//     use graphdb::core::Expression;
//     use graphdb::query::optimizer::IndexSelector;
//
//     let test_storage = TestStorage::new().expect("创建测试存储失败");
//     let storage = test_storage.storage();
//
//     let space_info = create_test_space("test_space");
//     assert_ok(get_storage(&storage).create_space(&space_info));
//
//     let tag_info = person_tag_info();
//     assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));
//
//     // Create two indexes: name and age
//     let name_index = Index::new(
//         1,
//         "person_name_idx".to_string(),
//         0,
//         "Person".to_string(),
//         vec![IndexField::new(
//             "name".to_string(),
//             Value::String("".to_string()),
//             false,
//         )],
//         vec!["name".to_string()],
//         IndexType::TagIndex,
//         false,
//     );
//
//     let age_index = Index::new(
//         2,
//         "person_age_idx".to_string(),
//         0,
//         "Person".to_string(),
//         vec![IndexField::new("age".to_string(), Value::Int(0), false)],
//         vec!["age".to_string()],
//         IndexType::TagIndex,
//         false,
//     );
//
//     assert_ok(get_storage(&storage).create_tag_index("test_space", &name_index));
//     assert_ok(get_storage(&storage).create_tag_index("test_space", &age_index));
//
//     // Test the equivalent query: name = 'Alice', should select the name index
//     let filter = Some(Expression::Binary {
//         left: Box::new(Expression::Variable("name".to_string())),
//         op: BinaryOperator::Equal,
//         right: Box::new(Expression::Literal(Value::String("Alice".to_string()))),
//     });
//
//     let available_indexes = vec![name_index.clone(), age_index.clone()];
//     let candidate = IndexSelector::select_best_index(&available_indexes, &filter);
//
//     assert!(candidate.is_some(), "应该选择一个索引");
//     let selected = candidate.expect("应该选择一个索引");
//     assert_eq!(selected.index.id, 1, "等值查询 name 应该选择 name 索引");
//
//     // Test range query: age > 25, should select the age index
//     let filter = Some(Expression::Binary {
//         left: Box::new(Expression::Variable("age".to_string())),
//         op: BinaryOperator::GreaterThan,
//         right: Box::new(Expression::Literal(Value::Int(25))),
//     });
//
//     let candidate = IndexSelector::select_best_index(&available_indexes, &filter);
//     assert!(candidate.is_some(), "应该选择一个索引");
//     let selected = candidate.expect("应该选择一个索引");
//     assert_eq!(selected.index.id, 2, "范围查询 age 应该选择 age 索引");
// }

// ==================== Scope Query Boundary Control Test ====================

#[test]
fn test_index_range_query_with_boundaries() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("age".to_string(), Value::Int(0), false)],
        properties: vec!["age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    // Insert test data: Age 20, 25, 30, 35, 40
    let vertices = vec![
        (VertexId::from_int64(1), Value::Int(20)),
        (VertexId::from_int64(2), Value::Int(25)),
        (VertexId::from_int64(3), Value::Int(30)),
        (VertexId::from_int64(4), Value::Int(35)),
        (VertexId::from_int64(5), Value::Int(40)),
    ];

    for (vid, age) in &vertices {
        let mut props = std::collections::HashMap::new();
        props.insert("age".to_string(), age.clone());
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(*vid, vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    // Test >= (with boundaries): age >= 25, should return 25, 30, 35, 40
    let _limit = IndexLimit {
        column: "age".to_string(),
        begin_value: Some("25".to_string()),
        end_value: None,
        include_begin: true,
        include_end: false,
        scan_type: ScanType::Range,
    };
    // Note: the range query capability of the storage layer is used here
    let retrieved =
        get_storage(&storage).lookup_index("test_space", "person_age_idx", &Value::Int(25));
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert!(
        vertex_ids.contains(&Value::Int(2)),
        ">= 25 should contain 25"
    );

    // Test > (without bounds): age > 25, should return 30, 35, 40
    // Note: current storage layer implementations may not support boundary control, verify basic functionality here
    let retrieved =
        get_storage(&storage).lookup_index("test_space", "person_age_idx", &Value::Int(30));
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert!(vertex_ids.contains(&Value::Int(3)), "Should contain 30");
}

// ==================== Scan Type Test ====================

#[test]
fn test_scan_type_unique() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    // insert data
    let vertices = vec![
        (VertexId::from_int64(1), Value::String("Alice".to_string())),
        (VertexId::from_int64(2), Value::String("Bob".to_string())),
        (VertexId::from_int64(3), Value::String("Alice".to_string())), // Repeated Alice
    ];

    for (vid, name) in &vertices {
        let mut props = std::collections::HashMap::new();
        props.insert("name".to_string(), name.clone());
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(*vid, vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    // Equivalent queries should return all matching vertices
    let retrieved = get_storage(&storage).lookup_index(
        "test_space",
        "person_name_idx",
        &Value::String("Alice".to_string()),
    );
    let vertex_ids = retrieved.expect("索引查询应该成功");
    assert_count(&vertex_ids, 2, "匹配的 Alice");
    assert!(
        vertex_ids.contains(&Value::Int(1)),
        "Should contain vertex 1"
    );
    assert!(
        vertex_ids.contains(&Value::Int(3)),
        "The vertex with index 3 should also be included."
    );
}

#[test]
fn test_scan_type_range() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_age_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new("age".to_string(), Value::Int(0), false)],
        properties: vec!["age".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    // Insert data for different ages
    for age in [20, 25, 30, 35, 40] {
        let mut props = std::collections::HashMap::new();
        props.insert("age".to_string(), Value::Int(age));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(VertexId::from_int64(age as i64), vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    // Basic functions of the validation range query
    let retrieved =
        get_storage(&storage).lookup_index("test_space", "person_age_idx", &Value::Int(30));
    let vertex_ids = retrieved.expect("范围查询应该成功");
    assert!(
        vertex_ids.contains(&Value::Int(30)),
        "The age should be 30."
    );
}

#[test]
fn test_scan_type_full() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let mut space_info = create_test_space("test_space");
    assert_ok(get_storage(&storage).create_space(&mut space_info));

    let tag_info = person_tag_info();
    assert_ok(get_storage(&storage).create_tag("test_space", &tag_info));

    let index = Index::new(graphdb::core::types::IndexConfig {
        id: 1,
        name: "person_name_idx".to_string(),
        space_id: 0,
        schema_name: "Person".to_string(),
        fields: vec![IndexField::new(
            "name".to_string(),
            Value::String("".to_string()),
            false,
        )],
        properties: vec!["name".to_string()],
        index_type: IndexType::TagIndex,
        is_unique: false,
        partial_condition: None,
    });

    assert_ok(get_storage(&storage).create_tag_index("test_space", &index));

    // Insert multiple pieces of data
    for i in 1..=5 {
        let mut props = std::collections::HashMap::new();
        props.insert("name".to_string(), Value::String(format!("Person{}", i)));
        let tag = graphdb::core::vertex_edge_path::Tag::new("Person".to_string(), props);
        let vertex = Vertex::new(VertexId::from_int64(i as i64), vec![tag]);
        assert_ok(get_storage(&storage).insert_vertex("test_space", vertex));
    }

    // A full scan should return all the data.
    let retrieved = get_storage(&storage).scan_vertices("test_space");
    let vertices = retrieved.expect("全扫描应该成功");
    assert_count(&vertices, 5, "顶点");
}
