//! Mapping from the raw `[vector]` configuration surface to engine types.
//!
//! Two consumers need the same translation: the backend assembly path
//! (`VectorBackend`, `assemble_local_engine`) and the transport-independent
//! API crate, which re-exports these helpers for its callers. It lives here
//! because `linkrs-sync` is the highest crate that may depend on both
//! `linkrs-config` and `simvec` without an upward dependency.

use std::path::PathBuf;
use std::sync::Arc;

use linkrs_config::{
    LocalVectorConfig, QdrantCollectionConfig, QuantizationSettings, VectorConfig,
};

/// Map raw TOML IVF settings to the local engine's IVF configuration.
pub fn local_ivf_config(local: &LocalVectorConfig) -> Option<simvec::IvfConfig> {
    let s = local.ivf.as_ref()?;
    Some(simvec::IvfConfig {
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
///
/// A TOML value of `0` leaves `usize` fields at the engine default; `None`
/// fields are passed through unchanged so "unset" stays distinguishable from
/// an explicit value.
pub fn local_hnsw_config(local: &LocalVectorConfig) -> Option<simvec::HnswConfig> {
    let s = local.hnsw.as_ref()?;
    let defaults = simvec::HnswConfig::default();
    Some(simvec::HnswConfig {
        m: if s.m > 0 { s.m } else { defaults.m },
        ef_construct: if s.ef_construct > 0 {
            s.ef_construct
        } else {
            defaults.ef_construct
        },
        full_scan_threshold: (s.full_scan_threshold > 0).then_some(s.full_scan_threshold),
        max_indexing_threads: (s.max_indexing_threads > 0).then_some(s.max_indexing_threads),
        on_disk: s.on_disk,
        payload_m: (s.payload_m > 0).then_some(s.payload_m),
        ef_search: if s.ef_search > 0 {
            s.ef_search
        } else {
            defaults.ef_search
        },
        stale_rebuild_ratio: s.stale_rebuild_ratio,
        iterative_max_rounds: (s.iterative_max_rounds > 0).then_some(s.iterative_max_rounds),
        max_scan_tuples: (s.max_scan_tuples > 0).then_some(s.max_scan_tuples),
    })
}

/// Map a raw quantization block to the engine's quantization configuration.
///
/// Shared by the local engine and the remote collection defaults so both
/// backends read the same TOML block the same way. `enabled = false` or a
/// missing/unknown type yields `None` (exact `f32`).
pub fn quantization_config(settings: &QuantizationSettings) -> Option<simvec::QuantizationConfig> {
    if !settings.enabled {
        return None;
    }
    let kind = settings.quantization_type.as_deref()?.to_lowercase();
    let always_ram = settings.always_ram;
    match kind.as_str() {
        "scalar" => {
            let mut cfg = simvec::QuantizationConfig::scalar(settings.quantile.unwrap_or(0.99));
            if let Some(always_ram) = always_ram {
                cfg = cfg.with_always_ram(always_ram);
            }
            Some(cfg)
        }
        "binary" => {
            let mut cfg = simvec::QuantizationConfig::binary();
            if let Some(always_ram) = always_ram {
                cfg = cfg.with_always_ram(always_ram);
            }
            Some(cfg)
        }
        "product" | "pq" => {
            let ratio = match settings
                .compression
                .as_deref()
                .unwrap_or("x4")
                .to_lowercase()
                .as_str()
            {
                "x8" | "8" => simvec::CompressionRatio::X8,
                "x16" | "16" => simvec::CompressionRatio::X16,
                "x32" | "32" => simvec::CompressionRatio::X32,
                "x64" | "64" => simvec::CompressionRatio::X64,
                _ => simvec::CompressionRatio::X4,
            };
            let mut cfg = simvec::QuantizationConfig::product(ratio);
            if let Some(always_ram) = always_ram {
                cfg = cfg.with_always_ram(always_ram);
            }
            Some(cfg)
        }
        _ => None,
    }
}

/// Map raw TOML quantization settings to the local engine's configuration.
pub fn local_quantization_config(local: &LocalVectorConfig) -> Option<simvec::QuantizationConfig> {
    local.quantization.as_ref().and_then(quantization_config)
}

/// Resolve the local engine's storage tuning, filling in engine defaults for
/// every knob the configuration leaves unset.
pub fn local_storage_config(local: &LocalVectorConfig) -> simvec::StorageConfig {
    let mut config = simvec::StorageConfig::default();
    let Some(storage) = local.storage.as_ref() else {
        return config;
    };
    if let Some(value) = storage.wal_checkpoint_bytes {
        config.wal_checkpoint_bytes = value;
    }
    if let Some(value) = storage.compaction_threshold {
        config.compaction_threshold = value;
    }
    if let Some(value) = storage.meta_save_interval {
        config.meta_save_interval = value;
    }
    // Block first: the page size has to be a multiple of it, and the caller
    // may set only one of the two.
    if let Some(value) = storage.payload_block_size {
        config.payload_block_size = value;
    }
    if let Some(value) = storage.payload_page_size_bytes {
        config.payload_page_size = value;
    }
    config
}

/// Open the local engine with the configured storage tuning and inject the
/// configured tier defaults.
///
/// This is the single assembly path: server startup and the embedded factory
/// both route through it so a new knob cannot be wired into one and missed in
/// the other. Returns a message instead of logging so the caller decides how
/// to fail.
pub fn assemble_local_engine(
    vector: &VectorConfig,
    data_dir: PathBuf,
) -> Result<Arc<simvec::LocalVectorEngine>, String> {
    let storage = local_storage_config(&vector.local);
    let engine = simvec::LocalVectorEngine::open_with_storage(&data_dir, storage).map_err(|e| {
        format!(
            "failed to open local vector engine at {}: {e}",
            data_dir.display()
        )
    })?;
    if let Some(hnsw) = local_hnsw_config(&vector.local) {
        engine.set_default_hnsw_config(hnsw);
    }
    if let Some(ivf) = local_ivf_config(&vector.local) {
        engine.set_default_ivf_config(ivf);
    }
    if let Some(quantization) = local_quantization_config(&vector.local) {
        engine.set_default_quantization_config(quantization);
    }
    Ok(Arc::new(engine))
}

/// Creation-time defaults applied to a new collection.
///
/// Both backends take their baseline from here: the local engine through its
/// `default_*` setters, the remote backend through
/// [`QdrantEngineHandle::default_collection_config`]. A field left `None`
/// means "do not override the engine or server default".
#[derive(Debug, Clone, Default)]
pub struct CollectionDefaults {
    pub hnsw: Option<simvec::HnswConfig>,
    pub quantization: Option<simvec::QuantizationConfig>,
    pub shard_number: Option<usize>,
    pub replication_factor: Option<usize>,
    pub write_consistency_factor: Option<usize>,
    pub on_disk_payload: Option<bool>,
    pub vectors_on_disk: Option<bool>,
    pub max_points: Option<u64>,
    pub storage_hints: Option<simvec::StorageHints>,
}

impl CollectionDefaults {
    /// Build the defaults described by `[vector.qdrant.collection]`, with
    /// `[vector.limits].default_max_points` as the fallback cap.
    pub fn from_qdrant(collection: &QdrantCollectionConfig, default_max_points: u64) -> Self {
        let hnsw = collection.hnsw.as_ref().map(|raw| {
            let mut hnsw =
                simvec::HnswConfig::new(raw.m.unwrap_or(16), raw.ef_construct.unwrap_or(100));
            hnsw.full_scan_threshold = raw.full_scan_threshold.filter(|value| *value > 0);
            hnsw.max_indexing_threads = raw.max_indexing_threads.filter(|value| *value > 0);
            hnsw.on_disk = raw.on_disk;
            hnsw.payload_m = raw.payload_m.filter(|value| *value > 0);
            hnsw
        });
        let has_hints = collection.optimizers.is_some()
            || collection.wal.is_some()
            || collection.strict_mode_enabled.is_some();
        let storage_hints = has_hints.then(|| simvec::StorageHints {
            optimizers: collection
                .optimizers
                .as_ref()
                .map(|optimizers| simvec::OptimizersHints {
                    deleted_threshold: optimizers.deleted_threshold,
                    vacuum_min_vector_number: optimizers.vacuum_min_vector_number,
                    default_segment_number: optimizers.default_segment_number,
                    max_segment_size: optimizers.max_segment_size,
                    memmap_threshold: optimizers.memmap_threshold,
                    indexing_threshold: optimizers.indexing_threshold,
                    flush_interval_sec: optimizers.flush_interval_sec,
                    max_optimization_threads: optimizers.max_optimization_threads,
                }),
            wal: collection.wal.as_ref().map(|wal| simvec::WalHints {
                wal_capacity_mb: wal.wal_capacity_mb,
                wal_segments_ahead: wal.wal_segments_ahead,
            }),
            strict_mode_enabled: collection.strict_mode_enabled,
        });

        Self {
            hnsw,
            quantization: collection
                .quantization
                .as_ref()
                .and_then(quantization_config),
            shard_number: collection.shard_number.map(|value| value as usize),
            replication_factor: collection.replication_factor.map(|value| value as usize),
            write_consistency_factor: collection
                .write_consistency_factor
                .map(|value| value as usize),
            on_disk_payload: collection.on_disk_payload,
            vectors_on_disk: collection.vectors_on_disk,
            max_points: collection
                .max_points
                .or_else(|| (default_max_points > 0).then_some(default_max_points)),
            storage_hints,
        }
    }

    /// Overlay the defaults on a request, letting every value the caller
    /// already set win.
    pub fn apply(&self, config: &mut simvec::CollectionConfig) {
        if let Some(hnsw) = &self.hnsw {
            config.index = simvec::IndexConfig::Hnsw(hnsw.clone());
        }
        if config.quantization_config.is_none() {
            if let Some(quantization) = &self.quantization {
                config.quantization_config = Some(quantization.clone());
            }
        }
        config.shard_number = config.shard_number.or(self.shard_number);
        config.replication_factor = config.replication_factor.or(self.replication_factor);
        config.write_consistency_factor = config
            .write_consistency_factor
            .or(self.write_consistency_factor);
        config.on_disk_payload = config.on_disk_payload.or(self.on_disk_payload);
        config.vectors_on_disk = config.vectors_on_disk.or(self.vectors_on_disk);
        config.max_points = config.max_points.or(self.max_points);
        if config.storage_hints.is_none() {
            config.storage_hints = self.storage_hints.clone();
        }
    }
}
