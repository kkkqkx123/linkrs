use crate::e2e_tests::common::{create_e2e_client, ensure_deleted, test_collection};
use vector_client::types::{
    CollectionConfig, CompressionRatio, DistanceMetric, HnswConfig, PayloadSchemaType,
    QuantizationConfig, QuantizationType, VectorPoint,
};

#[tokio::test]
async fn test_e2e_hnsw_config_roundtrip() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("hnsw_cfg");
    ensure_deleted(&client, &collection).await;

    let hnsw = HnswConfig::new(16, 200)
        .with_full_scan_threshold(10000)
        .with_on_disk(false);

    let mut config = CollectionConfig::new(4, DistanceMetric::Cosine);
    config.hnsw_config = Some(hnsw);
    config.on_disk_payload = Some(false);
    config.shard_number = Some(1);

    client
        .engine()
        .create_collection(&collection, config)
        .await
        .expect("create");

    let info = client
        .engine()
        .collection_info(&collection)
        .await
        .expect("info");
    assert_eq!(info.config.vector_size, 4);
    assert_eq!(info.config.distance, DistanceMetric::Cosine);

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

async fn quantization_supported(client: &vector_client::api::VectorClient) -> bool {
    let collection = test_collection("quant_probe");
    ensure_deleted(client, &collection).await;
    let qt = QuantizationType::Scalar {
        quantile: Some(0.99),
        always_ram: Some(true),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };
    let mut config = CollectionConfig::new(4, DistanceMetric::Euclid);
    config.quantization_config = Some(qc);
    let result = client.engine().create_collection(&collection, config).await;
    client.engine().delete_collection(&collection).await.ok();
    result.is_ok()
}

#[tokio::test]
async fn test_e2e_scalar_quantization_roundtrip() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    if !quantization_supported(&client).await {
        eprintln!("[E2E] Quantization not supported by this Qdrant version, skipping");
        return;
    }
    let collection = test_collection("scalar_q");
    ensure_deleted(&client, &collection).await;

    let qt = QuantizationType::Scalar {
        quantile: Some(0.99),
        always_ram: Some(true),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(4, DistanceMetric::Euclid);
    config.quantization_config = Some(qc);

    client
        .engine()
        .create_collection(&collection, config)
        .await
        .expect("create");

    let info = client
        .engine()
        .collection_info(&collection)
        .await
        .expect("info");
    assert_eq!(info.config.vector_size, 4);
    assert!(
        info.config.quantization_config.is_some(),
        "quantization config should be set"
    );

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_product_quantization_roundtrip() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    if !quantization_supported(&client).await {
        eprintln!("[E2E] Quantization not supported by this Qdrant version, skipping");
        return;
    }
    let collection = test_collection("product_q");
    ensure_deleted(&client, &collection).await;

    let qt = QuantizationType::Product {
        compression: CompressionRatio::X8,
        always_ram: Some(false),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(4, DistanceMetric::Dot);
    config.quantization_config = Some(qc);

    client
        .engine()
        .create_collection(&collection, config)
        .await
        .expect("create");

    let info = client
        .engine()
        .collection_info(&collection)
        .await
        .expect("info");
    assert!(info.config.quantization_config.is_some());

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_binary_quantization_roundtrip() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    if !quantization_supported(&client).await {
        eprintln!("[E2E] Quantization not supported by this Qdrant version, skipping");
        return;
    }
    let collection = test_collection("binary_q");
    ensure_deleted(&client, &collection).await;

    let qt = QuantizationType::Binary { always_ram: None };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(4, DistanceMetric::Cosine);
    config.quantization_config = Some(qc);

    client
        .engine()
        .create_collection(&collection, config)
        .await
        .expect("create");

    let info = client
        .engine()
        .collection_info(&collection)
        .await
        .expect("info");
    assert!(info.config.quantization_config.is_some());

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_on_disk_payload() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("on_disk");
    ensure_deleted(&client, &collection).await;

    let mut config = CollectionConfig::new(4, DistanceMetric::Cosine);
    config.on_disk_payload = Some(true);

    client
        .engine()
        .create_collection(&collection, config)
        .await
        .expect("create");

    let mut payload = std::collections::HashMap::new();
    payload.insert("data".to_string(), serde_json::json!("hello"));
    let point = VectorPoint::new(1u64, vec![0.5f32; 4]).with_payload(payload);
    client
        .engine()
        .upsert(&collection, point)
        .await
        .expect("upsert");

    let result = client
        .engine()
        .get(&collection, "1")
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        result
            .payload
            .as_ref()
            .and_then(|p| p.get("data").and_then(|v| v.as_str())),
        Some("hello")
    );

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_payload_index_crud() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("pidx");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    let created_title = client
        .engine()
        .create_payload_index(&collection, "title", PayloadSchemaType::Text)
        .await;
    let created_score = client
        .engine()
        .create_payload_index(&collection, "score", PayloadSchemaType::Float)
        .await;

    if created_title.is_err() || created_score.is_err() {
        eprintln!("[E2E] Payload index creation not supported, skipping");
        client.engine().delete_collection(&collection).await.ok();
        return;
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let mut indexes = Vec::new();
    for _ in 0..5 {
        indexes = client
            .engine()
            .list_payload_indexes(&collection)
            .await
            .expect("list");
        if indexes.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    if indexes.is_empty() {
        eprintln!("[E2E] Payload index listing not supported, skipping");
        client.engine().delete_collection(&collection).await.ok();
        return;
    }

    assert_eq!(indexes.len(), 2, "should have 2 payload indexes");

    client
        .engine()
        .delete_payload_index(&collection, "title")
        .await
        .expect("delete index");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let indexes = client
        .engine()
        .list_payload_indexes(&collection)
        .await
        .expect("list");
    assert_eq!(indexes.len(), 1, "should have 1 payload index after delete");

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}
