use std::sync::Arc;

use crate::core::Value;

use super::*;

#[test]
fn test_vertex_cache_basic() {
    let cache = RecordCache::new();

    let key = VertexCacheKey::new(1, 100);
    let vertex = CachedVertex {
        internal_id: 100,
        external_id: "test_vertex".to_string(),
        properties: vec![("name".to_string(), Value::String("Alice".to_string()))],
        cached_at_ts: 0,
    };

    cache.insert_vertex(key, vertex);

    let cached = cache.get_vertex(&VertexCacheKey::new(1, 100), 0);
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().external_id, "test_vertex");
}

#[test]
fn test_cache_remove() {
    let cache = RecordCache::new();

    let key = VertexCacheKey::new(1, 100);
    let vertex = CachedVertex {
        internal_id: 100,
        external_id: "test".to_string(),
        properties: vec![],
        cached_at_ts: 0,
    };

    cache.insert_vertex(key, vertex);
    assert!(cache.get_vertex(&VertexCacheKey::new(1, 100), 0).is_some());

    cache.remove_vertex(&VertexCacheKey::new(1, 100));
    assert!(cache.get_vertex(&VertexCacheKey::new(1, 100), 0).is_none());
}

#[test]
fn test_cache_clear() {
    let cache = RecordCache::new();

    for i in 0..10u32 {
        let key = VertexCacheKey::new(1, i);
        let vertex = CachedVertex {
            internal_id: i,
            external_id: format!("v{}", i),
            properties: vec![],
            cached_at_ts: 0,
        };
        cache.insert_vertex(key, vertex);
    }

    cache.clear();

    assert!(cache.get_vertex(&VertexCacheKey::new(1, 0), 0).is_none());
    assert!(cache.get_vertex(&VertexCacheKey::new(1, 9), 0).is_none());
}

#[test]
fn test_id_index_cache() {
    let cache = RecordCache::new();

    cache.insert_id_index(1, "user_001", 100, 0);
    cache.insert_id_index(1, "user_002", 200, 0);
    cache.insert_id_index(2, "product_001", 300, 0);

    assert_eq!(cache.get_id_index(1, "user_001", 0), Some(100));
    assert_eq!(cache.get_id_index(1, "user_002", 0), Some(200));
    assert_eq!(cache.get_id_index(2, "product_001", 0), Some(300));
    assert_eq!(cache.get_id_index(1, "nonexistent", 0), None);

    cache.remove_id_index(1, "user_001");
    assert_eq!(cache.get_id_index(1, "user_001", 0), None);
    assert_eq!(cache.get_id_index(1, "user_002", 0), Some(200));
}

#[test]
fn test_cache_config_with_ttl() {
    use std::time::Duration;

    let config = RecordCacheConfig {
        max_memory: 1024 * 1024,
        memory_ratio: (70, 30),
        ttl: Some(Duration::from_secs(60)),
        tti: Some(Duration::from_secs(30)),
        high_priority_ratio: 0.0,
    };
    let cache = RecordCache::with_config(config);

    let key = VertexCacheKey::new(1, 100);
    let vertex = CachedVertex {
        internal_id: 100,
        external_id: "test".to_string(),
        properties: vec![],
        cached_at_ts: 0,
    };
    cache.insert_vertex(key, vertex);

    assert!(cache.get_vertex(&VertexCacheKey::new(1, 100), 0).is_some());
}

#[test]
fn test_high_priority_pool() {
    let config = RecordCacheConfig {
        max_memory: 1024 * 1024,
        memory_ratio: (70, 30),
        high_priority_ratio: 0.1,
        ..Default::default()
    };
    let cache = RecordCache::with_config(config);

    for i in 0..100u32 {
        cache.insert_id_index(1, &format!("id_{}", i), i, 0);
    }

    assert!(cache.get_id_index(1, "id_50", 0).is_some());
}

#[test]
fn test_batch_id_indexes() {
    let cache = RecordCache::new();

    cache.insert_id_index(1, "user_001", 100, 0);
    cache.insert_id_index(1, "user_002", 200, 0);
    cache.insert_id_index(2, "product_001", 300, 0);

    let keys: Vec<(u32, &str)> = vec![
        (1, "user_001"),
        (1, "user_002"),
        (2, "product_001"),
        (1, "nonexistent"),
    ];

    let mut results = Vec::new();
    let mut hits = 0usize;
    let mut misses = 0usize;

    for (label, id) in &keys {
        match cache.get_id_index(*label, id, 0) {
            Some(internal_id) => {
                hits += 1;
                results.push(Some(internal_id));
            }
            None => {
                misses += 1;
                results.push(None);
            }
        }
    }

    assert_eq!(results.len(), 4);
    assert_eq!(hits, 3);
    assert_eq!(misses, 1);
    assert_eq!(results[0], Some(100));
    assert_eq!(results[1], Some(200));
    assert_eq!(results[2], Some(300));
    assert_eq!(results[3], None);
}

#[test]
fn test_invalidate_by_label() {
    let cache = RecordCache::new();

    cache.insert_vertex(
        VertexCacheKey::new(1, 100),
        CachedVertex {
            internal_id: 100,
            external_id: "v1".to_string(),
            properties: vec![],
            cached_at_ts: 0,
        },
    );
    cache.insert_vertex(
        VertexCacheKey::new(2, 200),
        CachedVertex {
            internal_id: 200,
            external_id: "v2".to_string(),
            properties: vec![],
            cached_at_ts: 0,
        },
    );
    cache.insert_id_index(1, "user_001", 100, 0);
    cache.insert_id_index(2, "user_002", 200, 0);

    assert!(
        cache.get_vertex(&VertexCacheKey::new(1, 100), 0).is_some(),
        "Vertex 1,100 should be cached before invalidation"
    );
    assert!(
        cache.get_vertex(&VertexCacheKey::new(2, 200), 0).is_some(),
        "Vertex 2,200 should be cached before invalidation"
    );

    cache.invalidate_vertices_by_label(1);
    cache.invalidate_id_indexes_by_label(1);

    assert!(
        cache.get_vertex(&VertexCacheKey::new(1, 100), 0).is_none(),
        "Vertex 1,100 should be invalidated"
    );
    assert!(
        cache.get_vertex(&VertexCacheKey::new(2, 200), 0).is_some(),
        "Vertex 2,200 should still be cached"
    );
    assert_eq!(
        cache.get_id_index(1, "user_001", 0),
        None,
        "ID index 1,user_001 should be invalidated"
    );
    assert_eq!(
        cache.get_id_index(2, "user_002", 0),
        Some(200),
        "ID index 2,user_002 should still be cached"
    );
}

#[test]
fn test_timestamp_staleness() {
    let cache = RecordCache::new();

    let key = VertexCacheKey::new(1, 42);
    let vertex = CachedVertex {
        internal_id: 42,
        external_id: "fresh".to_string(),
        properties: vec![],
        cached_at_ts: 100,
    };
    cache.insert_vertex(key, vertex);

    // query_ts < cached_at_ts → miss (data from future)
    assert!(cache.get_vertex(&key, 50).is_none());

    // query_ts == cached_at_ts → hit
    assert!(cache.get_vertex(&key, 100).is_some());

    // query_ts > cached_at_ts → hit
    assert!(cache.get_vertex(&key, 200).is_some());
}

#[test]
fn test_id_index_timestamp_staleness() {
    let cache = RecordCache::new();

    cache.insert_id_index(1, "user", 42, 100);

    // query_ts < cached_at_ts → miss
    assert_eq!(cache.get_id_index(1, "user", 50), None);

    // query_ts == cached_at_ts → hit
    assert_eq!(cache.get_id_index(1, "user", 100), Some(42));

    // query_ts > cached_at_ts → hit
    assert_eq!(cache.get_id_index(1, "user", 200), Some(42));
}

#[test]
fn test_concurrent_cache_access() {
    use std::thread;

    let cache = Arc::new(RecordCache::new());
    let mut handles = vec![];

    for t in 0..4 {
        let cache = cache.clone();
        let handle = thread::spawn(move || {
            for i in 0..100u32 {
                let key = VertexCacheKey::new(t, i);
                let vertex = CachedVertex {
                    internal_id: i,
                    external_id: format!("t{}_v{}", t, i),
                    properties: vec![],
                    cached_at_ts: 0,
                };
                cache.insert_vertex(key, vertex);
                let _ = cache.get_vertex(&key, 0);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

#[test]
fn test_estimated_size_accuracy() {
    let vertex = CachedVertex {
        internal_id: 1,
        external_id: "test_vertex".to_string(),
        properties: vec![
            ("name".to_string(), Value::String("Alice".to_string())),
            ("age".to_string(), Value::Int(30)),
        ],
        cached_at_ts: 0,
    };

    let estimated = vertex.estimated_size();
    let base_size = std::mem::size_of::<CachedVertex>() as u32;
    let external_cap = vertex.external_id.capacity() as u32;
    let mut property_size = 0u32;
    for (name, value) in &vertex.properties {
        property_size += name.capacity() as u32;
        property_size += value.estimated_size() as u32;
    }

    assert_eq!(estimated, base_size + external_cap + property_size);
    assert!(estimated > 0);
    assert!(
        estimated < 1000,
        "Estimated size should be reasonable for small vertex"
    );
}

#[test]
fn test_memory_weighted_eviction() {
    let config = RecordCacheConfig {
        max_memory: 1024,
        ..Default::default()
    };
    let cache = RecordCache::with_config(config);

    for i in 0..100u32 {
        let key = VertexCacheKey::new(1, i);
        let vertex = CachedVertex {
            internal_id: i,
            external_id: format!("vertex_{}", i),
            properties: vec![("data".to_string(), Value::String("x".repeat(50)))],
            cached_at_ts: 0,
        };
        cache.insert_vertex(key, vertex);
    }

    // verify eviction happened (not all 100 entries are present)
    let mut found = 0;
    for i in 0..100u32 {
        if cache.get_vertex(&VertexCacheKey::new(1, i), 0).is_some() {
            found += 1;
        }
    }
    assert!(found < 100, "Cache should have evicted entries");
}

#[test]
fn test_memory_overflow_eviction() {
    let config = RecordCacheConfig {
        max_memory: 512,
        ..Default::default()
    };
    let cache = RecordCache::with_config(config);

    for i in 0..200u32 {
        let key = VertexCacheKey::new(1, i);
        let vertex = CachedVertex {
            internal_id: i,
            external_id: format!("v{}", i),
            properties: vec![("data".to_string(), Value::String("x".repeat(100)))],
            cached_at_ts: 0,
        };
        cache.insert_vertex(key, vertex);
    }

    // verify eviction happened (test would OOM if no eviction)
    let mut found = 0;
    for i in 0..200u32 {
        if cache.get_vertex(&VertexCacheKey::new(1, i), 0).is_some() {
            found += 1;
        }
    }
    assert!(found < 200, "Evictions should have occurred");
}
