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
    /// Graph build parallelism. `0` leaves the engine default; `1` builds
    /// sequentially; `n >= 2` inserts disjoint slot subsets concurrently.
    #[serde(default)]
    pub max_indexing_threads: usize,
    /// Whether the graph lives on disk. Local engine only documents the
    /// value: it keeps published graphs in memory and never spills them.
    #[serde(default)]
    pub on_disk: Option<bool>,
    /// Per-layer connection cap for filtered graphs. `0` derives it from `m`.
    #[serde(default)]
    pub payload_m: usize,
    /// Overwrite-upsert ratio after which the graph is rebuilt to recover
    /// recall. Absent disables the staleness trigger.
    #[serde(default)]
    pub stale_rebuild_ratio: Option<f64>,
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
            max_indexing_threads: 0,
            on_disk: None,
            payload_m: 0,
            stale_rebuild_ratio: None,
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

/// Storage tuning for the local engine (`[vector.local.storage]`).
///
/// These knobs describe the host IO profile (log size, checkpoint cadence,
/// payload page geometry) rather than one collection's layout, so they apply
/// to every collection under the engine root. Every field is optional: absent
/// keeps the engine default, so an existing configuration is unaffected.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalStorageSettings {
    #[serde(default)]
    pub wal_checkpoint_bytes: Option<u64>,
    #[serde(default)]
    pub compaction_threshold: Option<f64>,
    #[serde(default)]
    pub meta_save_interval: Option<u64>,
    #[serde(default)]
    pub payload_block_size: Option<usize>,
    #[serde(default)]
    pub payload_page_size_bytes: Option<usize>,
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
    #[serde(default)]
    pub storage: Option<LocalStorageSettings>,
}

/// Cross-backend limits (`[vector.limits]`).
///
/// Only backends that enforce a limit act on it: the local engine refuses
/// writes past `default_max_points`, the remote backend ignores it (its
/// capacity is governed server-side).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VectorLimitsConfig {
    /// Live-point cap applied to collections created without an explicit
    /// `max_points`. `0` disables the cap.
    #[serde(default)]
    pub default_max_points: u64,
}

impl Default for VectorLimitsConfig {
    fn default() -> Self {
        Self {
            default_max_points: 0,
        }
    }
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

/// Collection defaults applied when a logical index creates a qdrant
/// collection (`[vector.qdrant.collection]`).
///
/// Only consulted at creation time: an existing collection keeps the layout
/// the server reports, matching the fact that most qdrant storage knobs are
/// immutable after creation.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QdrantCollectionConfig {
    #[serde(default)]
    pub shard_number: Option<u32>,
    #[serde(default)]
    pub replication_factor: Option<u32>,
    #[serde(default)]
    pub write_consistency_factor: Option<u32>,
    /// Keep payloads on disk instead of in RAM.
    #[serde(default)]
    pub on_disk_payload: Option<bool>,
    /// Keep the dense vectors themselves on disk.
    #[serde(default)]
    pub vectors_on_disk: Option<bool>,
    /// Live-point admission cap; enforced by the local engine only.
    #[serde(default)]
    pub max_points: Option<u64>,
    #[serde(default)]
    pub hnsw: Option<QdrantHnswSettings>,
    #[serde(default)]
    pub quantization: Option<QuantizationSettings>,
    #[serde(default)]
    pub optimizers: Option<QdrantOptimizersSettings>,
    #[serde(default)]
    pub wal: Option<QdrantWalSettings>,
    /// Whether the server enforces strict-mode query limits.
    #[serde(default)]
    pub strict_mode_enabled: Option<bool>,
}

/// HNSW defaults for qdrant collections. Every field is optional so a caller
/// overrides one knob without restating the server defaults.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QdrantHnswSettings {
    #[serde(default)]
    pub m: Option<usize>,
    #[serde(default)]
    pub ef_construct: Option<usize>,
    #[serde(default)]
    pub full_scan_threshold: Option<usize>,
    #[serde(default)]
    pub max_indexing_threads: Option<usize>,
    #[serde(default)]
    pub on_disk: Option<bool>,
    #[serde(default)]
    pub payload_m: Option<usize>,
}

/// Segment-optimizer defaults for qdrant collections.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QdrantOptimizersSettings {
    #[serde(default)]
    pub deleted_threshold: Option<f64>,
    #[serde(default)]
    pub vacuum_min_vector_number: Option<u64>,
    #[serde(default)]
    pub default_segment_number: Option<u64>,
    #[serde(default)]
    pub max_segment_size: Option<u64>,
    #[serde(default)]
    pub memmap_threshold: Option<u64>,
    #[serde(default)]
    pub indexing_threshold: Option<u64>,
    #[serde(default)]
    pub flush_interval_sec: Option<u64>,
    #[serde(default)]
    pub max_optimization_threads: Option<u64>,
}

/// Write-ahead log defaults for qdrant collections.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QdrantWalSettings {
    #[serde(default)]
    pub wal_capacity_mb: Option<u64>,
    #[serde(default)]
    pub wal_segments_ahead: Option<u64>,
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
    /// Creation-time collection defaults.
    #[serde(default)]
    pub collection: QdrantCollectionConfig,
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
            collection: QdrantCollectionConfig::default(),
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
    pub collection: VectorCollectionConfig,
    /// Cross-backend admission limits.
    #[serde(default)]
    pub limits: VectorLimitsConfig,
    #[serde(default)]
    pub retention: OutboxRetentionConfig,
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
            .field("collection", &self.collection)
            .field("limits", &self.limits)
            .field("retention", &self.retention);
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
            collection: VectorCollectionConfig::default(),
            limits: VectorLimitsConfig::default(),
            retention: OutboxRetentionConfig::default(),
            #[cfg(feature = "embedding")]
            embedding: None,
            #[cfg(feature = "embedding")]
            embedding_query_preprocessor: None,
            #[cfg(feature = "rerank")]
            rerank: None,
        }
    }
}

// ── Validation ───────────────────────────────────────────────────────────
//
// Every check here is static: it can be answered without an embedding
// service, a live qdrant, or a concrete collection dimension. Anything that
// needs runtime state (vector size vs. embedding dimension, server version)
// stays where it already happens — engine assembly and collection creation.

impl HnswSettings {
    /// Validate the graph parameters using the same floor the engines apply
    /// (`ef_construct >= max(2*m, 4)`). A zero value means "engine default"
    /// and is validated against the default it will resolve to.
    pub fn validate(&self) -> Result<(), String> {
        let m = if self.m == 0 { default_hnsw_m() } else { self.m };
        let ef = if self.ef_construct == 0 {
            default_hnsw_ef_construct()
        } else {
            self.ef_construct
        };
        let min_ef = (2 * m).max(4);
        if ef < min_ef {
            return Err(format!(
                "hnsw.ef_construct {ef} must be >= max(2*m, 4) = {min_ef} (m = {m})"
            ));
        }
        if let Some(ratio) = self.stale_rebuild_ratio {
            if !(ratio > 0.0 && ratio < 1.0) {
                return Err(format!(
                    "hnsw.stale_rebuild_ratio {ratio} must be within (0, 1)"
                ));
            }
        }
        Ok(())
    }
}

impl IvfSettings {
    /// Validate the coarse-quantizer parameters.
    pub fn validate(&self) -> Result<(), String> {
        if self.lists > 0 && self.default_nprobe as u32 > self.lists {
            return Err(format!(
                "ivf.default_nprobe {} must not exceed ivf.lists {}",
                self.default_nprobe, self.lists
            ));
        }
        if self.lists > 0 && self.max_probes > 0 && self.max_probes as u32 > self.lists {
            return Err(format!(
                "ivf.max_probes {} must not exceed ivf.lists {}",
                self.max_probes, self.lists
            ));
        }
        if self.drift_threshold <= 0.0 || self.drift_threshold >= 1.0 {
            return Err(format!(
                "ivf.drift_threshold {} must be within (0, 1)",
                self.drift_threshold
            ));
        }
        Ok(())
    }
}

impl QuantizationSettings {
    /// Validate the quantization selector. A disabled block is always valid;
    /// an enabled one must name a supported type and carry only the
    /// parameters that type understands.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let kind = self
            .quantization_type
            .as_deref()
            .map(str::to_lowercase)
            .unwrap_or_default();
        match kind.as_str() {
            "scalar" => {
                if let Some(quantile) = self.quantile {
                    if !(quantile > 0.0 && quantile <= 1.0) {
                        return Err(format!(
                            "quantization.quantile {quantile} must be within (0, 1]"
                        ));
                    }
                }
                if self.compression.is_some() {
                    return Err(
                        "quantization.compression is only valid for product quantization"
                            .to_string(),
                    );
                }
            }
            "binary" => {
                if self.compression.is_some() {
                    return Err(
                        "quantization.compression is only valid for product quantization"
                            .to_string(),
                    );
                }
            }
            "product" | "pq" => {
                if let Some(compression) = self.compression.as_deref() {
                    let normalized = compression.to_lowercase();
                    if !matches!(
                        normalized.as_str(),
                        "x4" | "4" | "x8" | "8" | "x16" | "16" | "x32" | "32" | "x64" | "64"
                    ) {
                        return Err(format!(
                            "quantization.compression '{compression}' must be one of \
                             x4/x8/x16/x32/x64"
                        ));
                    }
                }
            }
            "" => {
                return Err(
                    "quantization.enabled is true but quantization.quantization_type is missing"
                        .to_string(),
                )
            }
            other => {
                return Err(format!(
                    "unknown quantization.quantization_type '{other}'; expected \
                     scalar, binary or product"
                ))
            }
        }
        Ok(())
    }
}

impl LocalStorageSettings {
    /// Validate the storage tuning block against the engine's structural
    /// requirements (page geometry must divide evenly, ratios must be usable).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(bytes) = self.wal_checkpoint_bytes {
            if bytes == 0 {
                return Err("storage.wal_checkpoint_bytes must be >= 1".to_string());
            }
        }
        if let Some(threshold) = self.compaction_threshold {
            if !(threshold > 0.0 && threshold < 1.0) {
                return Err(format!(
                    "storage.compaction_threshold {threshold} must be within (0, 1)"
                ));
            }
        }
        if let Some(interval) = self.meta_save_interval {
            if interval == 0 {
                return Err("storage.meta_save_interval must be >= 1".to_string());
            }
        }
        if let Some(block) = self.payload_block_size {
            if block == 0 {
                return Err("storage.payload_block_size must be >= 1".to_string());
            }
        }
        if let Some(page) = self.payload_page_size_bytes {
            if page == 0 {
                return Err("storage.payload_page_size_bytes must be >= 1".to_string());
            }
            if let Some(block) = self.payload_block_size {
                if block > 0 && page % block != 0 {
                    return Err(format!(
                        "storage.payload_page_size_bytes {page} must be a multiple of \
                         storage.payload_block_size {block}"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl LocalVectorConfig {
    /// Validate every sub-block that is present.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(hnsw) = &self.hnsw {
            hnsw.validate()?;
        }
        if let Some(ivf) = &self.ivf {
            ivf.validate()?;
        }
        if let Some(quantization) = &self.quantization {
            quantization.validate()?;
        }
        if let Some(storage) = &self.storage {
            storage.validate()?;
        }
        Ok(())
    }
}

impl VectorLimitsConfig {
    /// Validate the admission limits.
    pub fn validate(&self) -> Result<(), String> {
        // `0` means "no cap"; every other value is a legal cap.
        Ok(())
    }
}

impl QdrantHnswSettings {
    /// Validate the graph parameters against the defaults they resolve to.
    pub fn validate(&self) -> Result<(), String> {
        let m = self.m.unwrap_or(default_hnsw_m());
        let ef = self.ef_construct.unwrap_or(default_hnsw_ef_construct());
        let min_ef = (2 * m).max(4);
        if ef < min_ef {
            return Err(format!(
                "collection.hnsw.ef_construct {ef} must be >= max(2*m, 4) = {min_ef} (m = {m})"
            ));
        }
        Ok(())
    }
}

impl QdrantOptimizersSettings {
    /// Validate the segment-optimizer block.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(threshold) = self.deleted_threshold {
            if !(threshold > 0.0 && threshold < 1.0) {
                return Err(format!(
                    "collection.optimizers.deleted_threshold {threshold} must be within (0, 1)"
                ));
            }
        }
        if self.max_segment_size == Some(0) {
            return Err("collection.optimizers.max_segment_size must be >= 1".to_string());
        }
        if self.indexing_threshold == Some(0) {
            return Err("collection.optimizers.indexing_threshold must be >= 1".to_string());
        }
        if self.memmap_threshold == Some(0) {
            return Err("collection.optimizers.memmap_threshold must be >= 1".to_string());
        }
        Ok(())
    }
}

impl QdrantWalSettings {
    /// Validate the write-ahead-log block.
    pub fn validate(&self) -> Result<(), String> {
        if self.wal_capacity_mb == Some(0) {
            return Err("collection.wal.wal_capacity_mb must be >= 1".to_string());
        }
        Ok(())
    }
}

impl QdrantCollectionConfig {
    /// Validate the creation-time collection defaults.
    pub fn validate(&self) -> Result<(), String> {
        if self.shard_number == Some(0) {
            return Err("collection.shard_number must be >= 1".to_string());
        }
        if self.replication_factor == Some(0) {
            return Err("collection.replication_factor must be >= 1".to_string());
        }
        if self.write_consistency_factor == Some(0) {
            return Err("collection.write_consistency_factor must be >= 1".to_string());
        }
        if let (Some(write), Some(replication)) =
            (self.write_consistency_factor, self.replication_factor)
        {
            if write > replication {
                return Err(format!(
                    "collection.write_consistency_factor {write} must not exceed \
                     collection.replication_factor {replication}"
                ));
            }
        }
        if self.max_points == Some(0) {
            return Err("collection.max_points must be >= 1 when set".to_string());
        }
        if let Some(hnsw) = &self.hnsw {
            hnsw.validate()?;
        }
        if let Some(quantization) = &self.quantization {
            quantization.validate()?;
        }
        if let Some(optimizers) = &self.optimizers {
            optimizers.validate()?;
        }
        if let Some(wal) = &self.wal {
            wal.validate()?;
        }
        Ok(())
    }
}

#[cfg(feature = "vector-qdrant")]
impl QdrantConfig {
    /// Validate the remote backend block, including the engine/backend
    /// enablement cross-check that previously only surfaced as a silently
    /// dropped vector subsystem.
    pub fn validate(&self, engine: VectorEngineKind) -> Result<(), String> {
        if engine == VectorEngineKind::Qdrant && !self.enabled {
            return Err(
                "vector.engine is \"qdrant\" but vector.qdrant.enabled is false; enable the \
                 backend or set vector.engine = \"local\""
                    .to_string(),
            );
        }
        self.collection.validate()
    }
}

impl VectorConfig {
    /// Validate the whole vector section. Called from `Config::validate` so a
    /// bad value fails startup instead of degrading the subsystem at runtime.
    pub fn validate(&self) -> Result<(), String> {
        self.local.validate()?;
        self.limits.validate()?;
        #[cfg(feature = "vector-qdrant")]
        self.qdrant.validate(self.engine)?;
        Ok(())
    }
}
