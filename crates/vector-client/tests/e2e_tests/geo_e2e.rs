use crate::e2e_tests::common::{create_e2e_client, ensure_deleted, test_collection};
use vector_client::types::{
    CollectionConfig, DistanceMetric, FilterCondition, GeoPoint, GeoRadius, PayloadSchemaType,
    SearchQuery, VectorFilter, VectorPoint,
};

#[tokio::test]
async fn test_e2e_geo_radius_filter() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("geo_radius");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    client
        .engine()
        .create_payload_index(&collection, "location", PayloadSchemaType::Geo)
        .await
        .expect("create geo index");

    let points = vec![
        VectorPoint::new(1u64, vec![0.1f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 48.8566, "lon": 2.3522}),
            );
            p
        }),
        VectorPoint::new(2u64, vec![0.2f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 51.5074, "lon": -0.1278}),
            );
            p
        }),
        VectorPoint::new(3u64, vec![0.3f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 35.6762, "lon": 139.6503}),
            );
            p
        }),
        VectorPoint::new(4u64, vec![0.4f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": -33.8688, "lon": 151.2093}),
            );
            p
        }),
    ];
    client
        .engine()
        .upsert_batch(&collection, points)
        .await
        .expect("upsert batch");

    let center = GeoPoint::new(48.8566, 2.3522);
    let geo_filter = FilterCondition::geo_radius("location", GeoRadius::new(center, 500.0));
    let filter = VectorFilter::new().must(geo_filter);
    let query = SearchQuery::new(vec![0.15f32; 4], 10).with_filter(filter);

    let results = client
        .engine()
        .search(&collection, query)
        .await
        .expect("search");
    assert!(
        results.len() <= 2,
        "at most Paris and London should be within 500km, got {}",
        results.len()
    );

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_geo_bounding_box_filter() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("geo_bbox");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    client
        .engine()
        .create_payload_index(&collection, "location", PayloadSchemaType::Geo)
        .await
        .expect("create geo index");

    let points = vec![
        VectorPoint::new(1u64, vec![0.1f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 48.8566, "lon": 2.3522}),
            );
            p
        }),
        VectorPoint::new(2u64, vec![0.2f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 51.5074, "lon": -0.1278}),
            );
            p
        }),
        VectorPoint::new(3u64, vec![0.3f32; 4]).with_payload({
            let mut p = std::collections::HashMap::new();
            p.insert(
                "location".to_string(),
                serde_json::json!({"lat": 35.6762, "lon": 139.6503}),
            );
            p
        }),
    ];
    client
        .engine()
        .upsert_batch(&collection, points)
        .await
        .expect("upsert batch");

    let bbox = vector_client::types::GeoBoundingBox::new(
        GeoPoint::new(55.0, -10.0),
        GeoPoint::new(45.0, 10.0),
    );
    let geo_filter = FilterCondition::geo_bounding_box("location", bbox);
    let filter = VectorFilter::new().must(geo_filter);
    let query = SearchQuery::new(vec![0.15f32; 4], 10).with_filter(filter);

    let results = client
        .engine()
        .search(&collection, query)
        .await
        .expect("search");
    assert_eq!(
        results.len(),
        2,
        "Paris and London should be in bounding box"
    );

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}
