//! The Linkrs API module
//!
//! Provides the transport-independent core API consumed by the network
//! service layer (`linkrs-server`) and library users.

pub mod api_core;

// Online migration drain fence shared by embedded and server callers.
// Only compiled when the `migration` feature is on so the minimal embedded
// closure drops the migration crate entirely.
#[cfg(feature = "migration")]
pub mod migration_online;

// Transaction-aware session variable store shared by the server and
// embedded session implementations.
pub mod session_variables;

#[cfg(feature = "embedded")]
pub mod embedded;

// ── Core re-exports ──────────────────────────────────────────────
pub use api_core::{CoreError, CoreResult, QueryApi, SchemaApi, SyncApi};

#[cfg(feature = "fulltext")]
pub use api_core::FulltextApi;
#[cfg(feature = "vector")]
pub use api_core::{VectorApi, VectorSearchResult};

// Hybrid retrieval helpers shared by fulltext and vector callers. Vector
// point IDs carry index suffixes while fulltext doc IDs do not, so fusion
// requires explicit normalization first.
pub use linkrs_core::fusion::{
    normalize_vector_hits, normalize_vector_point_id, rrf_fuse, weighted_fuse,
};
pub use linkrs_core::types::search::SearchHit;

// Validated hybrid fusion contract: the single fusion entry used by
// `Session::hybrid_search`; callers needing manual fusion must go through
// it (or `fuse_fulltext_and_vector` on the query crate) rather than wiring
// the raw core helpers by hand.
#[cfg(all(feature = "fulltext", feature = "vector"))]
pub use linkrs_query::{FusionStrategy, HybridFusionRequest};

/// Mapping helpers between raw linkrs-config settings and simvec
/// types; keeps the two crates decoupled from each other.
#[cfg(feature = "vector")]
pub mod vector_config {
    //! Thin re-exports of the canonical mapping in `linkrs_sync::vector_defaults`.
    //!
    //! The mapping lives below this crate so the backend assembly path and the
    //! API surface cannot drift apart; these wrappers keep the historical
    //! `linkrs_api::vector_config::*` paths working for callers.

    use linkrs_config::LocalVectorConfig;

    /// Map raw TOML IVF settings to the local engine's IVF configuration.
    pub fn local_ivf_config(local: &LocalVectorConfig) -> Option<simvec::IvfConfig> {
        linkrs_sync::vector_defaults::local_ivf_config(local)
    }

    /// Map raw TOML HNSW settings to the local engine's HNSW configuration.
    pub fn local_hnsw_config(local: &LocalVectorConfig) -> Option<simvec::HnswConfig> {
        linkrs_sync::vector_defaults::local_hnsw_config(local)
    }

    /// Map raw TOML quantization settings to the local engine's configuration.
    pub fn local_quantization_config(
        local: &LocalVectorConfig,
    ) -> Option<simvec::QuantizationConfig> {
        linkrs_sync::vector_defaults::local_quantization_config(local)
    }

    /// Resolve the local engine's storage tuning with engine defaults filled in.
    pub fn local_storage_config(local: &LocalVectorConfig) -> simvec::StorageConfig {
        linkrs_sync::vector_defaults::local_storage_config(local)
    }
}

#[cfg(all(test, feature = "vector"))]
mod vector_config_tests {
    use super::vector_config::{local_hnsw_config, local_ivf_config, local_quantization_config};
    use linkrs_config::{HnswSettings, IvfSettings, LocalVectorConfig};

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
            quantization: Some(linkrs_config::QuantizationSettings {
                enabled: true,
                quantization_type: Some("scalar".to_string()),
                quantile: Some(0.95),
                compression: None,
                always_ram: Some(false),
            }),
            ..LocalVectorConfig::default()
        };
        let qc = local_quantization_config(&local).unwrap();
        assert_eq!(qc.quantile_or_default(), 0.95);
        assert!(!qc.always_ram());

        let local = LocalVectorConfig {
            quantization: Some(linkrs_config::QuantizationSettings {
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
            quantization: Some(linkrs_config::QuantizationSettings {
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
