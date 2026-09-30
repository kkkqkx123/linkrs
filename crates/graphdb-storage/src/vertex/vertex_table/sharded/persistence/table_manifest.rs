//! Table manifest: shard layout pin that global IDs decode with.

use std::path::Path;

use super::super::ShardedVertexTable;
use super::common::MANIFEST_FORMAT_VERSION;
use graphdb_core::StorageResult;

/// Table-level manifest file pinning the shard layout that global internal
/// IDs were encoded with. Global IDs embed the shard count in their bit
/// layout, so opening a table persisted with a different shard count would
/// silently mis-decode every ID. The manifest makes that a loud error.
pub(crate) const TABLE_MANIFEST_FILE_NAME: &str = "table_manifest.json";

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct TableManifest {
    pub(crate) format_version: u8,
    pub(crate) label: graphdb_core::types::LabelId,
    pub(crate) label_name: String,
    pub(crate) num_shards: usize,
    pub(crate) segment_slots_bits: u32,
    pub(crate) total_segments: u32,
    /// Routing scheme version that encoded the persisted global IDs (see
    /// `ROUTER_VERSION`). Decoding with any other scheme would misroute
    /// every row, so unknown versions refuse the open.
    pub(crate) router_version: u8,
    /// Redistribution generation of this table lineage. Fresh tables start
    /// at zero; each offline redistribution bumps it. The commit manifest
    /// carries the same number so the open path can refuse a checkpoint
    /// mixed in from another generation instead of mis-decoding it.
    pub(crate) generation: u64,
    /// Wall-clock milliseconds of the last full baseline flush, written on
    /// every full flush and preserved across incremental flushes. Drives
    /// the baseline-age signal across restarts; a missing timestamp refuses
    /// the open and requires a rebuild.
    pub(crate) last_full_flush_ms: Option<u64>,
    pub(crate) checksum: u32,
}

pub(crate) struct TableManifestInput<'a> {
    pub(crate) format_version: u8,
    pub(crate) label: graphdb_core::types::LabelId,
    pub(crate) label_name: &'a str,
    pub(crate) num_shards: usize,
    pub(crate) segment_slots_bits: u32,
    pub(crate) total_segments: u32,
    pub(crate) router_version: u8,
    pub(crate) generation: u64,
    pub(crate) last_full_flush_ms: Option<u64>,
}

pub(crate) fn table_manifest_checksum(input: TableManifestInput<'_>) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[input.format_version]);
    hasher.update(&input.label.to_le_bytes());
    hasher.update(input.label_name.as_bytes());
    hasher.update(&(input.num_shards as u64).to_le_bytes());
    hasher.update(&input.segment_slots_bits.to_le_bytes());
    hasher.update(&input.total_segments.to_le_bytes());
    hasher.update(&[input.router_version]);
    hasher.update(&input.generation.to_le_bytes());
    hasher.update(&input.last_full_flush_ms.unwrap_or(0).to_le_bytes());
    hasher.finalize()
}

pub(crate) fn verify_table_manifest(manifest: &TableManifest, path: &Path) -> StorageResult<()> {
    if manifest.last_full_flush_ms.is_none() {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "table manifest at {} has no baseline timestamp: rebuild the table with the \
             offline redistribution tool instead of opening it in place",
            path.display(),
        )));
    }
    if manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "unsupported table manifest version {} at {}, expected {}: \
             the shard layout format changed; rebuild the table with the \
             offline redistribution tool instead of opening it in place",
            manifest.format_version,
            path.display(),
            MANIFEST_FORMAT_VERSION,
        )));
    }
    let expected = table_manifest_checksum(TableManifestInput {
        format_version: manifest.format_version,
        label: manifest.label,
        label_name: &manifest.label_name,
        num_shards: manifest.num_shards,
        segment_slots_bits: manifest.segment_slots_bits,
        total_segments: manifest.total_segments,
        router_version: manifest.router_version,
        generation: manifest.generation,
        last_full_flush_ms: manifest.last_full_flush_ms,
    });
    if expected != manifest.checksum {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "table manifest checksum mismatch at {}: expected {:#010x}, got {:#010x}",
            path.display(),
            expected,
            manifest.checksum,
        )));
    }
    let layout = super::super::routing::ShardLayout {
        num_shards: manifest.num_shards,
        segment_slots_bits: manifest.segment_slots_bits,
        total_segments: manifest.total_segments,
    };
    if manifest.router_version != super::super::routing::ROUTER_VERSION {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "unsupported table router version {} at {}, expected {}: \
             the external-key routing scheme changed; rebuild the table with the \
             offline redistribution tool instead of opening it in place",
            manifest.router_version,
            path.display(),
            super::super::routing::ROUTER_VERSION,
        )));
    }
    if !layout.is_consistent() {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "table manifest at {} pins an inconsistent shard layout \
             (num_shards={}, segment_slots_bits={}, total_segments={}): \
             rebuild the table with the offline redistribution tool \
             instead of opening it in place",
            path.display(),
            manifest.num_shards,
            manifest.segment_slots_bits,
            manifest.total_segments,
        )));
    }
    Ok(())
}

/// Manifest-pinned identity of one persisted table lineage: the shard
/// layout global IDs decode with, the routing scheme version that placed
/// them, and the redistribution generation they belong to. The open path
/// adopts all three; anything less would mis-decode or mix generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManifestLineage {
    pub(crate) layout: super::super::routing::ShardLayout,
    pub(crate) router_version: u8,
    pub(crate) generation: u64,
}

impl ShardedVertexTable {
    /// In-memory baseline timestamp as persisted form: `None` while no
    /// full flush ever ran. Incremental flushes preserve this value so the
    /// cross-restart age signal only moves on full baselines.
    fn persisted_baseline_ms(&self) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let raw = self.last_full_flush_ms.load(Ordering::Acquire);
        if raw == 0 {
            None
        } else {
            Some(raw)
        }
    }

    pub(crate) fn write_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let format_version = MANIFEST_FORMAT_VERSION;
        let last_full_flush_ms = self.persisted_baseline_ms();
        let checksum = table_manifest_checksum(TableManifestInput {
            format_version,
            label: self.label,
            label_name: &self.label_name,
            num_shards: self.layout.num_shards,
            segment_slots_bits: self.layout.segment_slots_bits,
            total_segments: self.layout.total_segments,
            router_version: super::super::routing::ROUTER_VERSION,
            generation: self.generation,
            last_full_flush_ms,
        });
        let manifest = TableManifest {
            format_version,
            label: self.label,
            label_name: self.label_name.clone(),
            num_shards: self.layout.num_shards,
            segment_slots_bits: self.layout.segment_slots_bits,
            total_segments: self.layout.total_segments,
            router_version: super::super::routing::ROUTER_VERSION,
            generation: self.generation,
            last_full_flush_ms,
            checksum,
        };
        let payload = serde_json::to_vec(&manifest)
            .map_err(|e| graphdb_core::StorageError::serialize_error(e.to_string()))?;
        crate::compression::write_shadow_file(
            path.as_ref().join(TABLE_MANIFEST_FILE_NAME),
            &payload,
        )
    }

    /// Shard layout pinned in the table manifest at `path`, if any.
    ///
    /// Opening a table adopts this lineage: the running configuration's
    /// shard count only applies to newly created tables. A missing manifest
    /// yields `None` (the caller keeps its configured layout and the strict
    /// load below refuses the open); an unknown version, router, or checksum
    /// failure errors with a rebuild directive instead of auto-migrating.
    pub(crate) fn manifest_layout<P: AsRef<Path>>(
        path: P,
    ) -> StorageResult<Option<ManifestLineage>> {
        let Some(manifest) = Self::read_table_manifest(&path)? else {
            return Ok(None);
        };
        Ok(Some(ManifestLineage {
            layout: super::super::routing::ShardLayout {
                num_shards: manifest.num_shards,
                segment_slots_bits: manifest.segment_slots_bits,
                total_segments: manifest.total_segments,
            },
            router_version: manifest.router_version,
            generation: manifest.generation,
        }))
    }

    pub(crate) fn read_table_manifest<P: AsRef<Path>>(
        path: P,
    ) -> StorageResult<Option<TableManifest>> {
        let manifest_path = path.as_ref().join(TABLE_MANIFEST_FILE_NAME);
        if !manifest_path.exists() {
            return Ok(None);
        }
        let payload = std::fs::read(&manifest_path)?;
        let manifest: TableManifest = serde_json::from_slice(&payload).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "invalid table manifest {}: {}",
                manifest_path.display(),
                e
            ))
        })?;
        verify_table_manifest(&manifest, &manifest_path)?;
        Ok(Some(manifest))
    }

    pub(crate) fn check_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let manifest = Self::read_table_manifest(&path)?;
        let Some(manifest) = manifest else {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "vertex table '{}' missing table manifest at {}: refusing open without shard layout pin",
                self.label_name,
                path.as_ref().join(TABLE_MANIFEST_FILE_NAME).display(),
            )));
        };
        if manifest.num_shards != self.layout.num_shards
            || manifest.segment_slots_bits != self.layout.segment_slots_bits
            || manifest.total_segments != self.layout.total_segments
        {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "vertex table '{}' persisted with layout (num_shards={}, segment_slots_bits={}, total_segments={}) \
                 but opened with layout (num_shards={}, segment_slots_bits={}, total_segments={}) \
                 (manifest {}): global internal IDs embed the shard layout and would \
                 mis-decode; reopen with vertex_table_shards={} or migrate the data with \
                 the offline redistribution tool",
                self.label_name,
                manifest.num_shards,
                manifest.segment_slots_bits,
                manifest.total_segments,
                self.layout.num_shards,
                self.layout.segment_slots_bits,
                self.layout.total_segments,
                path.as_ref().join(TABLE_MANIFEST_FILE_NAME).display(),
                manifest.num_shards,
            )));
        }
        if manifest.label != self.label {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "vertex table '{}' manifest label mismatch: manifest has label {} but table \
                 opened as label {}",
                self.label_name, manifest.label, self.label,
            )));
        }
        if manifest.generation != self.generation {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "vertex table '{}' persisted at redistribution generation {} but opened \
                 as generation {} (manifest {}): checkpoints from another lineage would \
                 mis-decode global IDs; open the directory with a table adopted from \
                 its own manifest instead of reusing this instance",
                self.label_name,
                manifest.generation,
                self.generation,
                path.as_ref().join(TABLE_MANIFEST_FILE_NAME).display(),
            )));
        }
        Ok(())
    }
}
