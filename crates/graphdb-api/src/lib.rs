//! The GraphDB API module
//!
//! Provides the transport-independent core API consumed by the network
//! service layer (`graphdb-server`) and library users.

pub mod api_core;

// Transaction-aware session variable store shared by the server and
// embedded session implementations.
pub mod session_variables;

#[cfg(feature = "embedded")]
pub mod embedded;

// ── Core re-exports ──────────────────────────────────────────────
pub use api_core::{CoreError, CoreResult, QueryApi, SchemaApi, SyncApi};

#[cfg(feature = "vector")]
pub use api_core::{VectorApi, VectorSearchResult};

/// Mapping helpers between raw graphdb-config settings and vector-search
/// types; keeps the two crates decoupled from each other.
#[cfg(feature = "vector")]
pub mod vector_config {
    use graphdb_config::LocalVectorConfig;

    /// Map raw TOML IVF settings to the local engine's IVF configuration.
    pub fn local_ivf_config(local: &LocalVectorConfig) -> Option<vector_search::IvfConfig> {
        let s = local.ivf.as_ref()?;
        Some(vector_search::IvfConfig {
            lists: if s.lists == 0 {
                None
            } else {
                Some(s.lists.max(1))
            },
            min_build_points: s.min_build_points,
            sample_limit: s.sample_limit,
            kmeans_max_iter: s.kmeans_max_iter,
            drift_threshold: s.drift_threshold,
            drift_check_interval: s.drift_check_interval,
            default_nprobe: s.default_nprobe,
            auto_promotion: s.auto_promotion,
            max_probes: if s.max_probes == 0 {
                None
            } else {
                Some(s.max_probes)
            },
        })
    }

    /// Map raw TOML HNSW settings to the local engine's HNSW configuration.
    /// A TOML value of `0` leaves the field at the engine default; nonzero
    /// values override it.
    pub fn local_hnsw_config(local: &LocalVectorConfig) -> Option<vector_search::HnswConfig> {
        let s = local.hnsw.as_ref()?;
        let defaults = vector_search::HnswConfig::default();
        Some(vector_search::HnswConfig {
            m: if s.m > 0 { s.m } else { defaults.m },
            ef_construct: if s.ef_construct > 0 {
                s.ef_construct
            } else {
                defaults.ef_construct
            },
            full_scan_threshold: (s.full_scan_threshold > 0).then_some(s.full_scan_threshold),
            ef_search: if s.ef_search > 0 {
                s.ef_search
            } else {
                defaults.ef_search
            },
            iterative_max_rounds: (s.iterative_max_rounds > 0).then_some(s.iterative_max_rounds),
            max_scan_tuples: (s.max_scan_tuples > 0).then_some(s.max_scan_tuples),
            ..defaults
        })
    }

    /// Map raw TOML quantization settings to the local engine's quantization config.
    ///
    /// Mirrors Qdrant's scalar/product/binary builders (`qdrant_features.md:4`);
    /// `enabled=false` or missing/unknown type yields `None` (exact f32).
    pub fn local_quantization_config(
        local: &LocalVectorConfig,
    ) -> Option<vector_search::QuantizationConfig> {
        let s = local.quantization.as_ref()?;
        if !s.enabled {
            return None;
        }
        let type_str = s.quantization_type.as_deref()?.to_lowercase();
        let always_ram = s.always_ram;
        match type_str.as_str() {
            "scalar" => {
                let quantile = s.quantile.unwrap_or(0.99);
                let mut cfg = vector_search::QuantizationConfig::scalar(quantile);
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                Some(cfg)
            }
            "binary" => {
                let mut cfg = vector_search::QuantizationConfig::binary();
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                Some(cfg)
            }
            "product" | "pq" => {
                let ratio = match s
                    .compression
                    .as_deref()
                    .unwrap_or("x4")
                    .to_lowercase()
                    .as_str()
                {
                    "x4" | "4" => vector_search::CompressionRatio::X4,
                    "x8" | "8" => vector_search::CompressionRatio::X8,
                    "x16" | "16" => vector_search::CompressionRatio::X16,
                    "x32" | "32" => vector_search::CompressionRatio::X32,
                    "x64" | "64" => vector_search::CompressionRatio::X64,
                    _ => vector_search::CompressionRatio::X4,
                };
                let mut cfg = vector_search::QuantizationConfig::product(ratio);
                if let Some(ar) = always_ram {
                    cfg = cfg.with_always_ram(ar);
                }
                Some(cfg)
            }
            _ => None,
        }
    }
}

#[cfg(all(test, feature = "vector"))]
mod vector_config_tests {
    use super::vector_config::{local_hnsw_config, local_ivf_config, local_quantization_config};
    use graphdb_config::{HnswSettings, IvfSettings, LocalVectorConfig};

    #[test]
    fn zero_toml_fields_map_to_engine_defaults() {
        let local = LocalVectorConfig {
            hnsw: Some(HnswSettings::default()),
            ivf: Some(IvfSettings::default()),
            ..LocalVectorConfig::default()
        };

        let hnsw = local_hnsw_config(&local).unwrap();
        assert_eq!(hnsw.iterative_max_rounds, None);
        assert_eq!(hnsw.max_scan_tuples, None);

        let ivf = local_ivf_config(&local).unwrap();
        assert_eq!(ivf.max_probes, None);
    }

    #[test]
    fn scan_limit_toml_fields_flow_through() {
        let local = LocalVectorConfig {
            hnsw: Some(HnswSettings {
                iterative_max_rounds: 5,
                max_scan_tuples: 20_000,
                ..HnswSettings::default()
            }),
            ivf: Some(IvfSettings {
                max_probes: 16,
                ..IvfSettings::default()
            }),
            ..LocalVectorConfig::default()
        };

        let hnsw = local_hnsw_config(&local).unwrap();
        assert_eq!(hnsw.iterative_max_rounds, Some(5));
        assert_eq!(hnsw.max_scan_tuples, Some(20_000));
        assert!(hnsw.validate().is_ok());

        let ivf = local_ivf_config(&local).unwrap();
        assert_eq!(ivf.max_probes, Some(16));
        assert!(ivf.validate().is_ok());
        assert_eq!(
            ivf.effective_max_probes(8),
            8,
            "probe cap clamped to list count"
        );
    }

    #[test]
    fn quantization_toml_maps_to_engine_config() {
        let local = LocalVectorConfig {
            quantization: Some(graphdb_config::QuantizationSettings {
                enabled: true,
                quantization_type: Some("scalar".to_string()),
                quantile: Some(0.95),
                compression: None,
                always_ram: Some(false),
            }),
            ..LocalVectorConfig::default()
        };
        let qc = local_quantization_config(&local).unwrap();
        assert!(qc.enabled);
        assert_eq!(qc.quantile_or_default(), 0.95);
        assert!(!qc.always_ram());

        let local = LocalVectorConfig {
            quantization: Some(graphdb_config::QuantizationSettings {
                enabled: true,
                quantization_type: Some("product".to_string()),
                quantile: None,
                compression: Some("x8".to_string()),
                always_ram: None,
            }),
            ..LocalVectorConfig::default()
        };
        let qc = local_quantization_config(&local).unwrap();
        assert!(qc.is_product());
        assert_eq!(qc.quant_bytes_per_vector(128), 64); // X8: 128*4/8=64
        assert!(qc.validate(128).is_ok());

        let disabled = LocalVectorConfig {
            quantization: Some(graphdb_config::QuantizationSettings {
                enabled: false,
                quantization_type: Some("scalar".to_string()),
                ..Default::default()
            }),
            ..LocalVectorConfig::default()
        };
        assert!(local_quantization_config(&disabled).is_none());
    }
}

#[cfg(feature = "embedded")]
pub use embedded::GraphDatabase;

pub mod storage {
    pub use graphdb_storage::*;

    #[cfg(test)]
    pub use graphdb_storage::MockStorage;
}
