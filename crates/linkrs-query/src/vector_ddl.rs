//! Shared semantic helpers for vector index DDL.
//!
//! `CREATE VECTOR INDEX` can arrive through two transports: the GQL
//! statement pipeline (parser → binder → planner → the vector operator)
//! and the HTTP / gRPC index endpoints, which carry structured options.
//! All three must build the engine configuration identically, so the
//! mapping lives here once and every entry point calls into it.

#[cfg(feature = "vector")]
use crate::parser::ast::vector::VectorDistance;
use crate::parser::ast::vector::{CompressionRatioKind, QuantizationKind};

/// Map a GQL distance token to the engine metric.
#[cfg(feature = "vector")]
pub fn distance_metric(distance: VectorDistance) -> simvec::DistanceMetric {
    match distance {
        VectorDistance::Cosine => simvec::DistanceMetric::Cosine,
        VectorDistance::Euclidean => simvec::DistanceMetric::Euclid,
        VectorDistance::Dot => simvec::DistanceMetric::Dot,
        VectorDistance::Manhattan => simvec::DistanceMetric::Manhattan,
    }
}

/// Parse a quantization kind from its wire/DSL spelling.
///
/// Accepts the same tokens as the GQL parser: `scalar`, `binary`,
/// `product`/`pq`, and `none`/`disabled`/`off` (which disable
/// quantization and therefore map to `None`).
pub fn quantization_kind_from_str(raw: &str) -> Option<Option<QuantizationKind>> {
    match raw.to_lowercase().as_str() {
        "none" | "disabled" | "off" => Some(None),
        "scalar" => Some(Some(QuantizationKind::Scalar)),
        "binary" => Some(Some(QuantizationKind::Binary)),
        "product" | "pq" => Some(Some(QuantizationKind::Product)),
        _ => None,
    }
}

/// Parse a product-quantization compression ratio (`x4`…`x64` or `4`…`64`).
pub fn compression_from_str(raw: &str) -> Option<CompressionRatioKind> {
    match raw.to_lowercase().as_str() {
        "x4" | "4" => Some(CompressionRatioKind::X4),
        "x8" | "8" => Some(CompressionRatioKind::X8),
        "x16" | "16" => Some(CompressionRatioKind::X16),
        "x32" | "32" => Some(CompressionRatioKind::X32),
        "x64" | "64" => Some(CompressionRatioKind::X64),
        _ => None,
    }
}

/// Structured options for building a collection configuration.
///
/// Storage-layout knobs (`shard_number`, `on_disk_payload`, …) are carried
/// only by the HTTP/gRPC entry points; the GQL statement pipeline leaves them
/// at `None` so the statement grammar does not grow transport-specific
/// keywords. Every field is optional: `None` means "engine or backend
/// default".
#[derive(Debug, Clone, Default)]
pub struct CollectionOptions {
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
    pub hnsw_full_scan_threshold: Option<usize>,
    pub hnsw_max_indexing_threads: Option<usize>,
    pub hnsw_on_disk: Option<bool>,
    pub hnsw_payload_m: Option<usize>,
    pub quantization: Option<QuantizationKind>,
    pub quantile: Option<f32>,
    pub compression: Option<CompressionRatioKind>,
    pub always_ram: Option<bool>,
    pub shard_number: Option<usize>,
    pub replication_factor: Option<usize>,
    pub write_consistency_factor: Option<usize>,
    pub on_disk_payload: Option<bool>,
    pub vectors_on_disk: Option<bool>,
    pub max_points: Option<u64>,
}

/// Build the engine collection config from DDL parameters.
///
/// This is the single source of truth for how `CREATE VECTOR INDEX`
/// options turn into a [`simvec::CollectionConfig`]; the query operator
/// and the HTTP / gRPC index endpoints all route through it so the
/// transports cannot drift from the statement pipeline.
#[cfg(feature = "vector")]
pub fn build_collection_config(
    vector_size: usize,
    distance: simvec::DistanceMetric,
    options: &CollectionOptions,
) -> Result<simvec::CollectionConfig, String> {
    let mut config = simvec::CollectionConfig::new(vector_size, distance);

    let has_hnsw = options.hnsw_m.is_some()
        || options.hnsw_ef_construct.is_some()
        || options.hnsw_full_scan_threshold.is_some()
        || options.hnsw_max_indexing_threads.is_some()
        || options.hnsw_on_disk.is_some()
        || options.hnsw_payload_m.is_some();
    if has_hnsw {
        let mut hnsw = simvec::HnswConfig::default();
        if let Some(m) = options.hnsw_m {
            hnsw.m = m;
        }
        if let Some(ef_construct) = options.hnsw_ef_construct {
            hnsw.ef_construct = ef_construct;
        }
        if let Some(threshold) = options.hnsw_full_scan_threshold {
            hnsw.full_scan_threshold = Some(threshold);
        }
        if let Some(threads) = options.hnsw_max_indexing_threads {
            hnsw.max_indexing_threads = Some(threads);
        }
        if let Some(on_disk) = options.hnsw_on_disk {
            hnsw.on_disk = Some(on_disk);
        }
        if let Some(payload_m) = options.hnsw_payload_m {
            hnsw.payload_m = Some(payload_m);
        }
        hnsw.validate()
            .map_err(|e| format!("invalid HNSW parameters: {}", e))?;
        config = config.with_hnsw(hnsw);
    }

    if let Some(kind) = options.quantization {
        if options.compression.is_some() && !matches!(kind, QuantizationKind::Product) {
            return Err(format!(
                "compression requires quantization=PRODUCT, got quantization={:?}; omit compression or use PRODUCT",
                kind
            ));
        }
        let cfg = match kind {
            QuantizationKind::Scalar => {
                let mut cfg = simvec::QuantizationConfig::scalar(options.quantile.unwrap_or(0.99));
                if let Some(always_ram) = options.always_ram {
                    cfg = cfg.with_always_ram(always_ram);
                }
                cfg
            }
            QuantizationKind::Binary => {
                let mut cfg = simvec::QuantizationConfig::binary();
                if let Some(always_ram) = options.always_ram {
                    cfg = cfg.with_always_ram(always_ram);
                }
                cfg
            }
            QuantizationKind::Product => {
                let ratio = match options.compression {
                    Some(CompressionRatioKind::X4) => simvec::CompressionRatio::X4,
                    Some(CompressionRatioKind::X8) => simvec::CompressionRatio::X8,
                    Some(CompressionRatioKind::X16) => simvec::CompressionRatio::X16,
                    Some(CompressionRatioKind::X32) => simvec::CompressionRatio::X32,
                    Some(CompressionRatioKind::X64) => simvec::CompressionRatio::X64,
                    None => simvec::CompressionRatio::X4,
                };
                let mut cfg = simvec::QuantizationConfig::product(ratio);
                if let Some(always_ram) = options.always_ram {
                    cfg = cfg.with_always_ram(always_ram);
                }
                cfg
            }
        };
        cfg.validate(vector_size)
            .map_err(|e| format!("invalid quantization parameters: {}", e))?;
        config = config.with_quantization(cfg);
    }

    // Storage layout: rejected here rather than at the engine so every
    // transport reports the same message.
    if options.shard_number == Some(0) {
        return Err("shard_number must be >= 1 when set".to_string());
    }
    if options.replication_factor == Some(0) {
        return Err("replication_factor must be >= 1 when set".to_string());
    }
    if options.write_consistency_factor == Some(0) {
        return Err("write_consistency_factor must be >= 1 when set".to_string());
    }
    if let (Some(write), Some(replication)) = (
        options.write_consistency_factor,
        options.replication_factor,
    ) {
        if write > replication {
            return Err(format!(
                "write_consistency_factor {write} must not exceed replication_factor {replication}"
            ));
        }
    }
    if options.max_points == Some(0) {
        return Err("max_points must be >= 1 when set".to_string());
    }

    config.shard_number = options.shard_number;
    config.replication_factor = options.replication_factor;
    config.write_consistency_factor = options.write_consistency_factor;
    config.on_disk_payload = options.on_disk_payload;
    config.vectors_on_disk = options.vectors_on_disk;
    config.max_points = options.max_points;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_vector_size_and_distance() {
        #[cfg(feature = "vector")]
        {
            let config = build_collection_config(
                128,
                simvec::DistanceMetric::Cosine,
                &CollectionOptions::default(),
            )
            .expect("config");
            assert_eq!(config.vector_size, 128);
            assert_eq!(config.shard_number, None);
        }
    }

    #[test]
    fn compression_without_product_quantization_is_rejected() {
        #[cfg(feature = "vector")]
        {
            let err = build_collection_config(
                128,
                simvec::DistanceMetric::Cosine,
                &CollectionOptions {
                    quantization: Some(QuantizationKind::Scalar),
                    compression: Some(CompressionRatioKind::X8),
                    ..CollectionOptions::default()
                },
            )
            .unwrap_err();
            assert!(err.contains("PRODUCT"));
        }
    }

    #[test]
    fn storage_options_land_on_the_config() {
        #[cfg(feature = "vector")]
        {
            let config = build_collection_config(
                64,
                simvec::DistanceMetric::Euclid,
                &CollectionOptions {
                    hnsw_m: Some(24),
                    hnsw_ef_construct: Some(128),
                    hnsw_on_disk: Some(true),
                    shard_number: Some(4),
                    replication_factor: Some(2),
                    write_consistency_factor: Some(1),
                    on_disk_payload: Some(true),
                    vectors_on_disk: Some(true),
                    max_points: Some(1_000_000),
                    ..CollectionOptions::default()
                },
            )
            .expect("config");
            assert_eq!(config.shard_number, Some(4));
            assert_eq!(config.replication_factor, Some(2));
            assert_eq!(config.write_consistency_factor, Some(1));
            assert_eq!(config.on_disk_payload, Some(true));
            assert_eq!(config.vectors_on_disk, Some(true));
            assert_eq!(config.max_points, Some(1_000_000));
            match config.index {
                simvec::IndexConfig::Hnsw(hnsw) => {
                    assert_eq!(hnsw.m, 24);
                    assert_eq!(hnsw.ef_construct, 128);
                    assert_eq!(hnsw.on_disk, Some(true));
                }
                other => panic!("expected HNSW, got {other:?}"),
            }
        }
    }

    #[test]
    fn write_consistency_above_replication_is_rejected() {
        #[cfg(feature = "vector")]
        {
            let err = build_collection_config(
                64,
                simvec::DistanceMetric::Cosine,
                &CollectionOptions {
                    replication_factor: Some(1),
                    write_consistency_factor: Some(2),
                    ..CollectionOptions::default()
                },
            )
            .unwrap_err();
            assert!(err.contains("must not exceed"));
        }
    }

    #[test]
    fn wire_tokens_parse() {
        assert_eq!(
            quantization_kind_from_str("scalar"),
            Some(Some(QuantizationKind::Scalar))
        );
        assert_eq!(quantization_kind_from_str("off"), Some(None));
        assert_eq!(quantization_kind_from_str("bogus"), None);
        assert_eq!(compression_from_str("X16"), Some(CompressionRatioKind::X16));
        assert_eq!(compression_from_str("32"), Some(CompressionRatioKind::X32));
        assert_eq!(compression_from_str("x3"), None);
    }
}
