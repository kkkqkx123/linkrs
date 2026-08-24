//! The GraphDB API module
//!
//! Provides the transport-independent core API consumed by the network
//! service layer (`graphdb-server`) and library users.

pub mod core;

// Transaction-aware session variable store shared by the server and
// embedded session implementations.
pub mod session_variables;

#[cfg(feature = "embedded")]
pub mod embedded;

// ── Core re-exports ──────────────────────────────────────────────
pub use core::{CoreError, CoreResult, QueryApi, SchemaApi, SyncApi};

#[cfg(feature = "vector")]
pub use core::{VectorApi, VectorSearchResult};

/// Mapping helpers between raw graphdb-config settings and vector-search
/// types; keeps the two crates decoupled from each other.
#[cfg(feature = "vector")]
pub mod vector_config {
    use graphdb_config::config::LocalVectorConfig;

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
        })
    }

    /// Map raw TOML HNSW settings to the local engine's HNSW configuration.
    /// `0` leaves a field at the engine default.
    pub fn local_hnsw_config(local: &LocalVectorConfig) -> Option<vector_search::HnswConfig> {
        let s = local.hnsw.as_ref()?;
        let mut config = vector_search::HnswConfig::default();
        if s.m > 0 {
            config.m = s.m;
        }
        if s.ef_construct > 0 {
            config.ef_construct = s.ef_construct;
        }
        if s.full_scan_threshold > 0 {
            config.full_scan_threshold = Some(s.full_scan_threshold);
        }
        if s.ef_search > 0 {
            config.ef_search = s.ef_search;
        }
        Some(config)
    }
}

#[cfg(feature = "embedded")]
pub use embedded::GraphDatabase;
