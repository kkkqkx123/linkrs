//! Vector search subsystem configuration (`[vector]`).
//!
//! Pure data only: engine selection, engine-local tuning surfaces and the
//! remote Qdrant endpoint description. Building a client from
//! [`QdrantConfig`] lives with the client crate's consumers so this crate
//! never depends on a vector engine implementation.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[cfg(feature = "embedding")]
use llm_embedding::{EmbeddingConfig, PreprocessorConfig};
#[cfg(feature = "rerank")]
use llm_rerank::{RerankConfig, RerankFusionStrategy};

/// Vector search engine kind
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorEngineKind {
    #[default]
    Local,
    #[cfg(feature = "vector-qdrant")]
    Qdrant,
}

/// IVF settings for the local vector engine (raw TOML surface).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IvfSettings {
    #[serde(default)]
    pub auto_promotion: bool,
    #[serde(default)]
    pub lists: u32,
    #[serde(default = "default_ivf_min_build_points")]
    pub min_build_points: u64,
    #[serde(default = "default_ivf_sample_limit")]
    pub sample_limit: usize,
    #[serde(default = "default_ivf_kmeans_max_iter")]
    pub kmeans_max_iter: u32,
    #[serde(default = "default_ivf_drift_threshold")]
    pub drift_threshold: f64,
    #[serde(default = "default_ivf_drift_check_interval")]
    pub drift_check_interval: u64,
    #[serde(default = "default_ivf_nprobe")]
    pub default_nprobe: usize,
    #[serde(default)]
    pub max_probes: usize,
}

impl Default for IvfSettings {
    fn default() -> Self {
        Self {
            auto_promotion: false,
            lists: 0,
            min_build_points: 100_000,
            sample_limit: 65_536,
            kmeans_max_iter: 10,
            drift_threshold: 0.10,
            drift_check_interval: 25_000,
            default_nprobe: 8,
            max_probes: 0,
        }
    }
}

fn default_ivf_min_build_points() -> u64 {
    100_000
}
fn default_ivf_sample_limit() -> usize {
    65_536
}
fn default_ivf_kmeans_max_iter() -> u32 {
    10
}
fn default_ivf_drift_threshold() -> f64 {
    0.10
}
fn default_ivf_drift_check_interval() -> u64 {
    25_000
}
fn default_ivf_nprobe() -> usize {
    8
}

/// HNSW settings for the local vector engine (raw TOML surface).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HnswSettings {
    #[serde(default = "default_hnsw_m")]
    pub m: usize,
    #[serde(default = "default_hnsw_ef_construct")]
    pub ef_construct: usize,
    #[serde(default)]
    pub full_scan_threshold: usize,
    #[serde(default)]
    pub ef_search: usize,
    #[serde(default)]
    pub iterative_max_rounds: usize,
    #[serde(default)]
    pub max_scan_tuples: u64,
}

impl Default for HnswSettings {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construct: 100,
            full_scan_threshold: 0,
            ef_search: 0,
            iterative_max_rounds: 0,
            max_scan_tuples: 0,
        }
    }
}

fn default_hnsw_m() -> usize {
    16
}
fn default_hnsw_ef_construct() -> usize {
    100
}

/// Quantization settings for the local vector engine (raw TOML surface).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QuantizationSettings {
    #[serde(default)]
    pub quantization_type: Option<String>,
    #[serde(default)]
    pub quantile: Option<f32>,
    #[serde(default)]
    pub compression: Option<String>,
    #[serde(default)]
    pub always_ram: Option<bool>,
    #[serde(default)]
    pub enabled: bool,
}

/// Local vector engine configuration
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct LocalVectorConfig {
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub hnsw: Option<HnswSettings>,
    #[serde(default)]
    pub ivf: Option<IvfSettings>,
    #[serde(default)]
    pub quantization: Option<QuantizationSettings>,
}

/// MVCC settings for vector search (default off).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct VectorMvccConfig {
    #[serde(default)]
    pub ssi_read_set: bool,
}

/// Collection granularity for vector indexes.
///
/// `Space` maps one space to one physical collection. `Field` keeps the
/// space-level collection but isolates `(tag, field)` tenants through a
/// group filter, matching the per-field isolation fulltext gets from
/// per-field directories.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorCollectionGranularity {
    #[default]
    Space,
    Field,
}

/// Collection settings for vector indexes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct VectorCollectionConfig {
    #[serde(default)]
    pub granularity: VectorCollectionGranularity,
}

/// Outbox retention settings.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutboxRetentionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_retention_interval")]
    pub prune_interval_secs: u64,
    #[serde(default = "default_retention_grace")]
    pub grace_lsn_distance: u64,
    #[serde(default = "default_retention_age_ms")]
    pub max_applied_age_ms: u64,
    #[serde(default = "default_retention_archive_rows")]
    pub max_archive_rows: u64,
}

fn default_retention_interval() -> u64 {
    3600
}
fn default_retention_grace() -> u64 {
    10_000
}
fn default_retention_age_ms() -> u64 {
    86_400_000
}
fn default_retention_archive_rows() -> u64 {
    100_000
}

impl Default for OutboxRetentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            prune_interval_secs: default_retention_interval(),
            grace_lsn_distance: default_retention_grace(),
            max_applied_age_ms: default_retention_age_ms(),
            max_archive_rows: default_retention_archive_rows(),
        }
    }
}

/// Optional post-recall rerank stage for vector text queries.
///
/// Absent disables rerank with zero overhead. Present but invalid values only
/// warn and disable at service assembly; startup never fails for rerank.
#[cfg(feature = "rerank")]
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VectorRerankConfig {
    /// Endpoint shared by rerank providers (address, model, timeout, proxy).
    #[serde(flatten)]
    pub endpoint: RerankConfig,
    /// Maximum recall candidates forwarded per call.
    #[serde(default = "default_rerank_max_candidates")]
    pub max_candidates: usize,
    /// Strategy fusing rerank scores with recall scores.
    #[serde(default)]
    pub fusion: RerankFusionStrategy,
    /// Payload field carrying candidate text. Defaults to the searched field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_field: Option<String>,
}

#[cfg(feature = "rerank")]
fn default_rerank_max_candidates() -> usize {
    20
}

#[cfg(feature = "rerank")]
impl VectorRerankConfig {
    /// Guardrail validation: endpoint address and model are required and the
    /// candidate window stays within a cost-bounded range.
    pub fn validate(&self) -> Result<(), String> {
        if self.endpoint.base_url.is_empty() {
            return Err("vector.rerank base_url must not be empty".to_string());
        }
        if !self.endpoint.base_url.contains("://") {
            return Err("vector.rerank base_url must include a scheme".to_string());
        }
        if self.endpoint.model.is_empty() {
            return Err("vector.rerank model must not be empty".to_string());
        }
        if self.max_candidates == 0 || self.max_candidates > 100 {
            return Err("vector.rerank max_candidates must be within 1..=100".to_string());
        }
        Ok(())
    }
}

/// Wire transport used to reach a Qdrant service.
///
/// Both transports may be compiled in at once; this selects the one used at
/// runtime. The default follows the historical behavior (gRPC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QdrantTransport {
    #[default]
    Grpc,
    Http,
}

/// Connection endpoint for a Qdrant service (pure data).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QdrantConnectionConfig {
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
    pub api_key: Option<String>,
    pub connect_timeout_secs: u64,
    pub http_port: Option<u16>,
    /// Which wire transport to use; requires the matching feature to be
    /// compiled in.
    #[serde(default)]
    pub transport: QdrantTransport,
}

impl Default for QdrantConnectionConfig {
    fn default() -> Self {
        // The default transport is gRPC (see [`QdrantTransport`]), so the
        // default port follows Qdrant's gRPC convention. The HTTP engine
        // falls back to its own conventional 6333 when `http_port` is unset.
        Self {
            host: "localhost".to_string(),
            port: 6334,
            use_tls: false,
            api_key: None,
            connect_timeout_secs: 5,
            http_port: None,
            transport: QdrantTransport::default(),
        }
    }
}

/// Request timeout budget for a Qdrant service (pure data).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QdrantTimeoutConfig {
    pub request_timeout_secs: u64,
    pub search_timeout_secs: u64,
    pub upsert_timeout_secs: u64,
}

impl Default for QdrantTimeoutConfig {
    fn default() -> Self {
        Self {
            request_timeout_secs: 30,
            search_timeout_secs: 60,
            upsert_timeout_secs: 30,
        }
    }
}

/// Remote Qdrant service configuration.
///
/// TOML surface matches the client crate's config so existing `[vector.qdrant]`
/// sections keep parsing; mapping into the client happens at the consumer.
///
/// Equality is intentionally not derived: the per-engine embedding endpoint
/// comes from another crate whose config type carries no equality semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QdrantConfig {
    pub enabled: bool,
    #[serde(default)]
    pub connection: QdrantConnectionConfig,
    #[serde(default)]
    pub timeout: QdrantTimeoutConfig,
    /// Per-engine embedding endpoint; overrides the shared
    /// `[vector.embedding]` when both are set.
    #[cfg(feature = "embedding")]
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
}

impl QdrantConfig {
    /// A disabled default (used when the `[vector.qdrant]` section is absent).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            connection: QdrantConnectionConfig::default(),
            timeout: QdrantTimeoutConfig::default(),
            #[cfg(feature = "embedding")]
            embedding: None,
        }
    }
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Vector search configuration
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct VectorConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub engine: VectorEngineKind,
    #[serde(default)]
    pub local: LocalVectorConfig,
    #[cfg(feature = "vector-qdrant")]
    #[serde(default)]
    pub qdrant: QdrantConfig,
    #[serde(default)]
    pub mvcc: VectorMvccConfig,
    #[serde(default)]
    pub collection: VectorCollectionConfig,
    #[serde(default)]
    pub retention: OutboxRetentionConfig,
    /// Optional write-time text-to-vector conversion.
    ///
    /// The write path only persists explicit vector columns by default.
    /// Text queries are a read-time convenience resolved through the
    /// embedding service. When enabled, vertex writes embed text fields
    /// that already have a vector index and stage the resulting vectors
    /// alongside the original properties. Explicit vectors win over
    /// auto-embedded values for the same field. Failures fail the staged
    /// write so deployments pay the embedding availability cost directly.
    #[serde(default)]
    pub auto_embed_text: bool,
    /// Backend-independent embedding endpoint shared by the local engine
    /// and the remote client. `[vector.qdrant.embedding]` overrides it
    /// when both are set.
    #[cfg(feature = "embedding")]
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    /// Optional query-side preprocessor override. The shared `embedding`
    /// config carries the document-side preprocessor used for writes;
    /// reads apply this override when set.
    #[cfg(feature = "embedding")]
    #[serde(default)]
    pub embedding_query_preprocessor: Option<PreprocessorConfig>,
    /// Optional post-recall rerank stage. Absent disables rerank with zero
    /// overhead; invalid values warn and disable at assembly.
    #[cfg(feature = "rerank")]
    #[serde(default)]
    pub rerank: Option<VectorRerankConfig>,
}

fn default_true() -> bool {
    true
}

/// Redact every credential-bearing sub-configuration wherever the vector
/// section is printed. Endpoint addresses stay visible; API keys do not.
impl fmt::Debug for VectorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("VectorConfig");
        debug
            .field("enabled", &self.enabled)
            .field("engine", &self.engine)
            .field("mvcc", &self.mvcc)
            .field("collection", &self.collection)
            .field("retention", &self.retention)
            .field("auto_embed_text", &self.auto_embed_text);
        #[cfg(feature = "embedding")]
        debug.field("embedding_configured", &self.embedding.is_some());
        #[cfg(feature = "vector-qdrant")]
        debug.field("qdrant_enabled", &self.qdrant.enabled);
        #[cfg(feature = "rerank")]
        debug.field("rerank_configured", &self.rerank.is_some());
        debug.finish()
    }
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            engine: VectorEngineKind::Local,
            local: LocalVectorConfig::default(),
            #[cfg(feature = "vector-qdrant")]
            qdrant: QdrantConfig::disabled(),
            mvcc: VectorMvccConfig::default(),
            collection: VectorCollectionConfig::default(),
            retention: OutboxRetentionConfig::default(),
            auto_embed_text: false,
            #[cfg(feature = "embedding")]
            embedding: None,
            #[cfg(feature = "embedding")]
            embedding_query_preprocessor: None,
            #[cfg(feature = "rerank")]
            rerank: None,
        }
    }
}
