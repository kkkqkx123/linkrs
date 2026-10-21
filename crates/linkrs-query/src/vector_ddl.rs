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
    hnsw_m: Option<usize>,
    hnsw_ef_construct: Option<usize>,
    quantization: Option<QuantizationKind>,
    quantile: Option<f32>,
    compression: Option<CompressionRatioKind>,
    always_ram: Option<bool>,
) -> Result<simvec::CollectionConfig, String> {
    let mut config = simvec::CollectionConfig::new(vector_size, distance);
    if hnsw_m.is_some() || hnsw_ef_construct.is_some() {
        let mut hnsw = simvec::HnswConfig::default();
        if let Some(m) = hnsw_m {
            hnsw.m = m;
        }
        if let Some(ef) = hnsw_ef_construct {
            hnsw.ef_construct = ef;
        }
        hnsw.validate()
            .map_err(|e| format!("invalid HNSW parameters: {}", e))?;
        config = config.with_hnsw(hnsw);
    }
    if let Some(kind) = quantization {
        if compression.is_some() && !matches!(kind, QuantizationKind::Product) {
            return Err(format!(
                "compression requires quantization=PRODUCT, got quantization={:?}; omit compression or use PRODUCT",
                kind
            ));
        }
        let cfg = match kind {
            QuantizationKind::Scalar => {
                let mut cfg = simvec::QuantizationConfig::scalar(quantile.unwrap_or(0.99));
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                cfg
            }
            QuantizationKind::Binary => {
                let mut cfg = simvec::QuantizationConfig::binary();
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                cfg
            }
            QuantizationKind::Product => {
                let ratio = match compression {
                    Some(CompressionRatioKind::X4) => simvec::CompressionRatio::X4,
                    Some(CompressionRatioKind::X8) => simvec::CompressionRatio::X8,
                    Some(CompressionRatioKind::X16) => simvec::CompressionRatio::X16,
                    Some(CompressionRatioKind::X32) => simvec::CompressionRatio::X32,
                    Some(CompressionRatioKind::X64) => simvec::CompressionRatio::X64,
                    None => simvec::CompressionRatio::X4,
                };
                let mut cfg = simvec::QuantizationConfig::product(ratio);
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                cfg
            }
        };
        cfg.validate(vector_size)
            .map_err(|e| format!("invalid quantization parameters: {}", e))?;
        config = config.with_quantization(cfg);
    }
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
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("config");
            assert_eq!(config.vector_size, 128);
        }
    }

    #[test]
    fn compression_without_product_quantization_is_rejected() {
        #[cfg(feature = "vector")]
        {
            let err = build_collection_config(
                128,
                simvec::DistanceMetric::Cosine,
                None,
                None,
                Some(QuantizationKind::Scalar),
                None,
                Some(CompressionRatioKind::X8),
                None,
            )
            .unwrap_err();
            assert!(err.contains("PRODUCT"));
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
