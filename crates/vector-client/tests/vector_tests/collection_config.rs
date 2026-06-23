use super::common::VectorTestContext;
use vector_client::types::{
    CollectionConfig, CompressionRatio, DistanceMetric, HnswConfig, QuantizationConfig,
    QuantizationType,
};

/// Create collection with HNSW config and verify via collection_info
#[tokio::test]
async fn test_create_collection_with_hnsw_config() {
    let ctx = VectorTestContext::with_dimension(128);

    let hnsw = HnswConfig::new(16, 200)
        .with_full_scan_threshold(10000)
        .with_max_indexing_threads(2)
        .with_on_disk(false);

    let mut config = CollectionConfig::new(128, DistanceMetric::Cosine);
    config.hnsw_config = Some(hnsw);
    config.on_disk_payload = Some(false);

    ctx.manager
        .create_index("hnsw_test", config.clone())
        .await
        .expect("create");

    let meta = ctx
        .manager
        .get_index_metadata("hnsw_test")
        .expect("metadata");
    let stored = &meta.config;
    assert_eq!(stored.vector_size, 128);
    assert_eq!(stored.distance, DistanceMetric::Cosine);

    let h = stored.hnsw_config.as_ref().expect("hnsw config");
    assert_eq!(h.m, 16);
    assert_eq!(h.ef_construct, 200);
    assert_eq!(h.full_scan_threshold, Some(10000));
    assert_eq!(h.max_indexing_threads, Some(2));
    assert_eq!(h.on_disk, Some(false));
    assert_eq!(stored.on_disk_payload, Some(false));
}

/// Create collection with Scalar quantization
#[tokio::test]
async fn test_create_collection_with_scalar_quantization() {
    let ctx = VectorTestContext::with_dimension(128);

    let qt = QuantizationType::Scalar {
        quantile: Some(0.99),
        always_ram: Some(true),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(128, DistanceMetric::Euclid);
    config.quantization_config = Some(qc);

    ctx.manager
        .create_index("scalar_quant_test", config.clone())
        .await
        .expect("create");

    let meta = ctx
        .manager
        .get_index_metadata("scalar_quant_test")
        .expect("metadata");
    let stored = &meta.config;
    let sq = stored.quantization_config.as_ref().expect("quant config");
    assert!(sq.enabled);
    let qt = sq.quant_type.as_ref().expect("quant type");
    match qt {
        QuantizationType::Scalar {
            quantile,
            always_ram,
        } => {
            assert!((quantile.unwrap_or(0.0) - 0.99).abs() < 0.001);
            assert_eq!(*always_ram, Some(true));
        }
        _ => panic!("expected Scalar quantization"),
    }
}

/// Create collection with Product quantization
#[tokio::test]
async fn test_create_collection_with_product_quantization() {
    let ctx = VectorTestContext::with_dimension(128);

    let qt = QuantizationType::Product {
        compression: CompressionRatio::X8,
        always_ram: Some(false),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(128, DistanceMetric::Dot);
    config.quantization_config = Some(qc);

    ctx.manager
        .create_index("product_quant_test", config.clone())
        .await
        .expect("create");

    let meta = ctx
        .manager
        .get_index_metadata("product_quant_test")
        .expect("metadata");
    let stored = &meta.config;
    let sq = stored.quantization_config.as_ref().expect("quant config");
    let qt = sq.quant_type.as_ref().expect("quant type");
    match qt {
        QuantizationType::Product {
            compression,
            always_ram,
        } => {
            assert_eq!(*compression, CompressionRatio::X8);
            assert_eq!(*always_ram, Some(false));
        }
        _ => panic!("expected Product quantization"),
    }
}

/// Create collection with Binary quantization
#[tokio::test]
async fn test_create_collection_with_binary_quantization() {
    let ctx = VectorTestContext::with_dimension(128);

    let qt = QuantizationType::Binary { always_ram: None };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(128, DistanceMetric::Cosine);
    config.quantization_config = Some(qc);

    ctx.manager
        .create_index("binary_quant_test", config.clone())
        .await
        .expect("create");

    let meta = ctx
        .manager
        .get_index_metadata("binary_quant_test")
        .expect("metadata");
    let stored = &meta.config;
    let sq = stored.quantization_config.as_ref().expect("quant config");
    let qt = sq.quant_type.as_ref().expect("quant type");
    match qt {
        QuantizationType::Binary { always_ram } => {
            assert_eq!(*always_ram, None);
        }
        _ => panic!("expected Binary quantization"),
    }
}

/// Create collection with all advanced options
#[tokio::test]
async fn test_create_collection_full_config() {
    let ctx = VectorTestContext::with_dimension(256);

    let hnsw = HnswConfig::new(32, 128)
        .with_full_scan_threshold(5000)
        .with_on_disk(true);
    let qt = QuantizationType::Scalar {
        quantile: None,
        always_ram: Some(true),
    };
    let qc = QuantizationConfig {
        enabled: true,
        quant_type: Some(qt),
    };

    let mut config = CollectionConfig::new(256, DistanceMetric::Euclid);
    config.hnsw_config = Some(hnsw);
    config.quantization_config = Some(qc);
    config.on_disk_payload = Some(true);
    config.shard_number = Some(2);
    config.replication_factor = Some(1);
    config.write_consistency_factor = Some(1);

    ctx.manager
        .create_index("full_config_test", config.clone())
        .await
        .expect("create");

    let meta = ctx
        .manager
        .get_index_metadata("full_config_test")
        .expect("metadata");
    let stored = &meta.config;
    assert_eq!(stored.vector_size, 256);
    assert_eq!(stored.distance, DistanceMetric::Euclid);
    assert!(stored.hnsw_config.is_some());
    assert!(stored.quantization_config.is_some());
    assert_eq!(stored.on_disk_payload, Some(true));
    assert_eq!(stored.shard_number, Some(2));
    assert_eq!(stored.replication_factor, Some(1));
    assert_eq!(stored.write_consistency_factor, Some(1));
}

/// Manhattan distance collection
#[tokio::test]
async fn test_create_collection_manhattan() {
    let ctx = VectorTestContext::with_dimension(64);

    let mut config = CollectionConfig::new(64, DistanceMetric::Manhattan);
    config.on_disk_payload = Some(false);

    ctx.manager
        .create_index("manhattan_test", config)
        .await
        .expect("create");
    assert!(ctx.manager.index_exists("manhattan_test"));
}
