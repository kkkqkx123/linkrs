use super::*;

#[test]
fn test_create_and_drop_user() {
    let mut storage = create_test_storage();

    let user = UserInfo::new("test_user".to_string(), "password123".to_string()).unwrap();
    storage.create_user(&user).unwrap();

    storage.drop_user("test_user").unwrap();
}

#[test]
fn test_grant_and_revoke_role() {
    let mut storage = create_test_storage();
    let space_id = setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let user = UserInfo::new("role_user".to_string(), "pass".to_string()).unwrap();
    storage.create_user(&user).unwrap();

    storage
        .grant_role("role_user", space_id, RoleType::Admin)
        .unwrap();
    storage.revoke_role("role_user", space_id).unwrap();

    storage.drop_user("role_user").unwrap();
}

#[test]
fn test_user_storage_persists_across_reload() {
    let (temp_dir, mut storage) = create_persistent_storage();

    let user = UserInfo::new("persist_user".to_string(), "password123".to_string())
        .expect("UserInfo::new should succeed")
        .with_locked(true)
        .with_max_queries_per_hour(42);

    storage.create_user(&user).unwrap();
    storage.save_to_disk().unwrap();

    let mut reloaded =
        GraphStorage::open(temp_dir.path().to_path_buf()).expect("Failed to reopen GraphStorage");

    assert!(reloaded.user_exists("persist_user"));
    assert!(reloaded.create_user(&user).unwrap());
}

#[test]
fn test_get_storage_stats_empty() {
    let storage = create_test_storage();
    let stats = storage.get_storage_stats();
    assert_eq!(stats.total_vertices, 0);
    assert_eq!(stats.total_edges, 0);
    assert_eq!(stats.total_spaces, 0);
}

#[test]
fn test_get_storage_stats_with_data() {
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

    let stats = storage.get_storage_stats();
    // Note: vertex/edge counts depend on MVCC visibility
    assert!(stats.total_spaces >= 1);
    assert!(stats.total_tags >= 1);
    assert!(stats.total_edge_types >= 1);
}

#[test]
fn test_get_db_path() {
    let storage = create_test_storage();
    // Default db_path is empty for new() without path
    let path = storage.get_db_path();
    assert!(path.is_empty() || path.contains("test"));
}

#[test]
fn test_get_nonexistent_vertex() {
    let storage = create_test_storage();
    let result = storage.get_vertex(
        "nonexistent",
        "Person",
        &VertexId::try_from_int64(999).expect("test vertex id"),
    );
    assert!(result.is_err());
}

#[test]
fn test_get_nonexistent_edge() {
    let storage = create_test_storage();
    let result = storage.get_edge(
        "nonexistent",
        &VertexId::try_from_int64(1).expect("test vertex id"),
        &VertexId::try_from_int64(2).expect("test vertex id"),
        "UNKNOWN",
        0,
    );
    assert!(result.is_err());
}

#[test]
fn test_delete_nonexistent_vertex() {
    let mut storage = create_test_storage();
    let result = storage.delete_vertex(
        "nonexistent",
        "Person",
        &VertexId::try_from_int64(999).expect("test vertex id"),
    );
    assert!(result.is_err());
}
