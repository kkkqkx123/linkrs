use super::common::{create_payload, create_test_points, generate_test_vectors, VectorTestContext};
use vector_client::types::{
    ConditionType, DistanceMetric, FilterCondition, GeoBoundingBox, GeoPoint, GeoRadius,
    MinShouldCondition, RangeCondition, SearchQuery, ValuesCountCondition, VectorFilter,
    VectorPoint,
};

/// Contains filter on array payload field
#[tokio::test]
async fn test_contains_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let payloads = [
        create_payload(vec![("tags", serde_json::json!(["rust", "db"]))]),
        create_payload(vec![("tags", serde_json::json!(["rust"]))]),
        create_payload(vec![("tags", serde_json::json!(["python", "ml"]))]),
    ];
    let ids = ["doc_1", "doc_2", "doc_3"];
    let points: Vec<VectorPoint> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| VectorPoint::new(id, vectors[i].clone()).with_payload(payloads[i].clone()))
        .collect();
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::contains("tags", "rust"));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 2, "should find 2 docs with rust tag");
    for r in &results {
        let id = r.id.to_string();
        assert!(id == "doc_1" || id == "doc_2", "unexpected result {}", id);
    }
}

/// HasId filter
#[tokio::test]
async fn test_has_id_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: [&str; 5] = ["a", "b", "c", "d", "e"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::has_id(vec![
        "a".into(),
        "c".into(),
        "e".into(),
    ]));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 3);
    for r in &results {
        let id = r.id.to_string();
        assert!(id == "a" || id == "c" || id == "e", "unexpected {}", id);
    }
}

/// MatchAny filter
#[tokio::test]
async fn test_match_any_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(4, 64, 42);
    let payloads = [
        create_payload(vec![("color", serde_json::json!("red"))]),
        create_payload(vec![("color", serde_json::json!("blue"))]),
        create_payload(vec![("color", serde_json::json!("green"))]),
        create_payload(vec![("color", serde_json::json!("red"))]),
    ];
    let ids = ["d1", "d2", "d3", "d4"];
    let points: Vec<VectorPoint> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| VectorPoint::new(id, vectors[i].clone()).with_payload(payloads[i].clone()))
        .collect();
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::match_any(
        "color",
        vec![serde_json::json!("red"), serde_json::json!("blue")],
    ));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 3);
}

/// IsEmpty filter
#[tokio::test]
async fn test_is_empty_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(2, 64, 42);
    let pts = vec![
        VectorPoint::new("d1", vectors[0].clone())
            .with_payload(create_payload(vec![("val", serde_json::json!(null))])),
        VectorPoint::new("d2", vectors[1].clone()),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::is_empty("val"));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 2);
}

/// IsNull filter
#[tokio::test]
async fn test_is_null_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(2, 64, 42);
    let pts = vec![
        VectorPoint::new("d1", vectors[0].clone())
            .with_payload(create_payload(vec![("val", serde_json::json!(42))])),
        VectorPoint::new("d2", vectors[1].clone()),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::is_null("val"));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id.to_string(), "d2");
}

/// Nested filter - use FilterCondition::new directly
#[tokio::test]
async fn test_nested_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(2, 64, 42);
    let pts = vec![
        VectorPoint::new("d1", vectors[0].clone()).with_payload(create_payload(vec![(
            "meta",
            serde_json::json!({"status": "active", "score": 100}),
        )])),
        VectorPoint::new("d2", vectors[1].clone()).with_payload(create_payload(vec![(
            "meta",
            serde_json::json!({"status": "inactive", "score": 50}),
        )])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let inner = VectorFilter::new().must(FilterCondition::match_value("status", "active"));
    let nested = FilterCondition::new(
        "meta",
        ConditionType::Nested {
            filter: Box::new(inner),
        },
    );
    let filter = VectorFilter::new().must(nested);
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id.to_string(), "d1");
}

/// GeoRadius filter
#[tokio::test]
async fn test_geo_radius_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let pts = vec![
        VectorPoint::new("paris", vectors[0].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 48.8566, "lon": 2.3522}),
        )])),
        VectorPoint::new("london", vectors[1].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 51.5074, "lon": -0.1278}),
        )])),
        VectorPoint::new("tokyo", vectors[2].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 35.6762, "lon": 139.6503}),
        )])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let center = GeoPoint::new(48.8566, 2.3522);
    let geo_radius = GeoRadius::new(center, 500.0);
    let filter = VectorFilter::new().must(FilterCondition::geo_radius("location", geo_radius));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(
        results.len(),
        2,
        "Paris and London should be within 500km of Paris center"
    );
}

/// GeoBoundingBox filter
#[tokio::test]
async fn test_geo_bounding_box_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let pts = vec![
        VectorPoint::new("paris", vectors[0].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 48.8566, "lon": 2.3522}),
        )])),
        VectorPoint::new("london", vectors[1].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 51.5074, "lon": -0.1278}),
        )])),
        VectorPoint::new("tokyo", vectors[2].clone()).with_payload(create_payload(vec![(
            "location",
            serde_json::json!({"lat": 35.6762, "lon": 139.6503}),
        )])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let bbox = GeoBoundingBox::new(GeoPoint::new(55.0, -10.0), GeoPoint::new(45.0, 10.0));
    let filter = VectorFilter::new().must(FilterCondition::geo_bounding_box("location", bbox));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(
        results.len(),
        2,
        "Paris and London should be in the bounding box"
    );
}

/// ValuesCount filter
#[tokio::test]
async fn test_values_count_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let pts = vec![
        VectorPoint::new("few", vectors[0].clone())
            .with_payload(create_payload(vec![("items", serde_json::json!([1]))])),
        VectorPoint::new("some", vectors[1].clone()).with_payload(create_payload(vec![(
            "items",
            serde_json::json!([1, 2, 3]),
        )])),
        VectorPoint::new("many", vectors[2].clone()).with_payload(create_payload(vec![(
            "items",
            serde_json::json!([1, 2, 3, 4, 5]),
        )])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let range = ValuesCountCondition::new().gte(2).lt(5);
    let filter = VectorFilter::new().must(FilterCondition::values_count("items", range));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id.to_string(), "some");
}

/// Should combinator with min_should
#[tokio::test]
async fn test_should_min_should_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let pts = vec![
        VectorPoint::new("d1", vectors[0].clone()).with_payload(create_payload(vec![
            ("a", serde_json::json!(1)),
            ("b", serde_json::json!(2)),
        ])),
        VectorPoint::new("d2", vectors[1].clone())
            .with_payload(create_payload(vec![("a", serde_json::json!(1))])),
        VectorPoint::new("d3", vectors[2].clone())
            .with_payload(create_payload(vec![("c", serde_json::json!(3))])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter {
        should: Some(vec![
            FilterCondition::range("a", RangeCondition::new().gte(1.0)),
            FilterCondition::range("b", RangeCondition::new().gte(1.0)),
        ]),
        min_should: Some(MinShouldCondition {
            conditions: vec![],
            min_count: 2,
        }),
        ..Default::default()
    };
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id.to_string(), "d1");
}

/// MustNot filter
#[tokio::test]
async fn test_must_not_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(3, 64, 42);
    let pts = vec![
        VectorPoint::new("d1", vectors[0].clone())
            .with_payload(create_payload(vec![("cat", serde_json::json!("tech"))])),
        VectorPoint::new("d2", vectors[1].clone())
            .with_payload(create_payload(vec![("cat", serde_json::json!("news"))])),
        VectorPoint::new("d3", vectors[2].clone())
            .with_payload(create_payload(vec![("cat", serde_json::json!("tech"))])),
    ];
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must_not(FilterCondition::match_value("cat", "news"));
    let query = SearchQuery::new(vectors[0].clone(), 10).with_filter(filter);
    let results = ctx
        .manager
        .search("space_1_Doc_emb", query)
        .await
        .expect("search");

    assert_eq!(results.len(), 2);
    for r in &results {
        assert_ne!(r.id.to_string(), "d2");
    }
}
