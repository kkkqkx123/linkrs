//! Commit manifest: one checkpoint pin written atomically last.

use std::path::{Path, PathBuf};

use super::super::ShardedVertexTable;
use super::common::{now_ms, MANIFEST_FORMAT_VERSION};
use super::sidecar::{collect_sidecar_records, sweep_unpinned_sidecars, SnapshotSidecarRecord};
use graphdb_core::StorageResult;

/// Commit manifest pinning one checkpoint of this table. Written atomically
/// after all shard files, it is the only commit point the recovery path
/// trusts: files outside the manifest are never read, and a manifest-listed
/// file that is missing or corrupt refuses the open instead of running sick.
pub(crate) const COMMIT_MANIFEST_FILE_NAME: &str = "commit_manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CommitKind {
    Full,
    Incremental,
}

impl CommitKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CommitManifest {
    pub(crate) format_version: u8,
    pub(crate) epoch: u64,
    pub(crate) kind: CommitKind,
    pub(crate) base_epoch: Option<u64>,
    /// Redistribution generation of the table lineage this checkpoint
    /// belongs to. Must match the table manifest generation: a checkpoint
    /// mixed in from another generation would mis-decode global IDs.
    pub(crate) generation: u64,
    pub(crate) files: Vec<String>,
    /// Derived eviction sidecars pinned for verification only. Absent decodes
    /// as empty; never part of the strict file set, so a missing or corrupt
    /// sidecar never refuses the open.
    #[serde(default)]
    pub(crate) sidecars: Vec<SnapshotSidecarRecord>,
    pub(crate) written_at_ms: u64,
    pub(crate) checksum: u32,
}

pub(crate) struct CommitManifestInput<'a> {
    pub(crate) format_version: u8,
    pub(crate) epoch: u64,
    pub(crate) kind: CommitKind,
    pub(crate) base_epoch: Option<u64>,
    pub(crate) generation: u64,
    pub(crate) files: &'a [String],
    pub(crate) sidecars: &'a [SnapshotSidecarRecord],
    pub(crate) written_at_ms: u64,
}

pub(crate) fn commit_manifest_checksum(input: CommitManifestInput<'_>) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[input.format_version]);
    hasher.update(&input.epoch.to_le_bytes());
    hasher.update(input.kind.as_str().as_bytes());
    hasher.update(&input.base_epoch.unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(&input.generation.to_le_bytes());
    for file in input.files {
        hasher.update(file.as_bytes());
        hasher.update(&[0]);
    }
    for sidecar in input.sidecars {
        hasher.update(sidecar.file.as_bytes());
        hasher.update(&[0]);
        hasher.update(&sidecar.bytes.to_le_bytes());
        hasher.update(&sidecar.checksum.to_le_bytes());
    }
    hasher.update(&input.written_at_ms.to_le_bytes());
    hasher.finalize()
}

pub(crate) fn verify_commit_manifest_content(
    manifest: &CommitManifest,
    path: &Path,
) -> StorageResult<()> {
    if manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "unsupported commit manifest version {} at {}, expected {}",
            manifest.format_version,
            path.display(),
            MANIFEST_FORMAT_VERSION,
        )));
    }
    let expected = commit_manifest_checksum(CommitManifestInput {
        format_version: manifest.format_version,
        epoch: manifest.epoch,
        kind: manifest.kind,
        base_epoch: manifest.base_epoch,
        generation: manifest.generation,
        files: &manifest.files,
        sidecars: &manifest.sidecars,
        written_at_ms: manifest.written_at_ms,
    });
    if expected != manifest.checksum {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "commit manifest checksum mismatch at {}: expected {:#010x}, got {:#010x}",
            path.display(),
            expected,
            manifest.checksum,
        )));
    }
    Ok(())
}

/// Damage classification for the graded open path. Fatal defects refuse the
/// open. Isolatable defects mark one column unavailable and keep the table
/// open for the healthy columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CorruptionClass {
    Fatal,
    Isolatable,
}

impl CorruptionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Isolatable => "isolatable",
        }
    }
}

pub(crate) fn commit_manifest_path(dir: &Path) -> PathBuf {
    dir.join(COMMIT_MANIFEST_FILE_NAME)
}

/// Column owning a manifest-listed file, when the file is per-column state
/// that can degrade to column-unavailable instead of refusing the whole
/// table. Overflow sidecars (`shard_N/<col>.overflow`) and incremental column
/// pages (`shard_N/columns_pages/<col>_<page>.page`) are isolatable; every
/// other checkpoint file stays table-critical.
pub(crate) fn column_for_isolatable_file(rel: &str) -> Option<String> {
    let (_, rest) = rel.split_once('/')?;
    if let Some(stem) = rest.strip_suffix(".overflow") {
        if stem.is_empty() || stem.contains('/') {
            return None;
        }
        return Some(stem.to_string());
    }
    let pages_prefix = "columns_pages/";
    if let Some(file) = rest.strip_prefix(pages_prefix) {
        let stem = file.strip_suffix(".page")?;
        if stem.contains('/') {
            return None;
        }
        let (col, page) = stem.rsplit_once('_')?;
        if col.is_empty() || page.parse::<usize>().is_err() {
            return None;
        }
        return Some(col.to_string());
    }
    None
}

fn collect_committed_files(dir: &Path) -> StorageResult<Vec<String>> {
    fn visit(root: &Path, cur: &Path, out: &mut Vec<String>) -> StorageResult<()> {
        for entry in std::fs::read_dir(cur)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, out)?;
            } else if path.is_file() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.ends_with(".tmp") || name == COMMIT_MANIFEST_FILE_NAME {
                    continue;
                }
                // Checkpoint sidecars are derived mmap caches, not
                // authoritative state: excluded so a missing or pruned
                // sidecar never refuses the open. Reload re-evicts from
                // whatever sidecars exist and keeps the rest resident.
                if name.ends_with(".snapshot") {
                    continue;
                }
                let rel = path.strip_prefix(root).map_err(|e| {
                    graphdb_core::StorageError::invalid_operation(format!(
                        "table file outside its root {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if dir.exists() {
        visit(dir, dir, &mut out)?;
    }
    out.sort();
    Ok(out)
}

impl ShardedVertexTable {
    pub(crate) fn read_commit_manifest<P: AsRef<Path>>(
        path: P,
    ) -> StorageResult<Option<CommitManifest>> {
        let manifest_path = commit_manifest_path(path.as_ref());
        if !manifest_path.exists() {
            return Ok(None);
        }
        let payload = std::fs::read(&manifest_path)?;
        let manifest: CommitManifest = serde_json::from_slice(&payload).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "invalid commit manifest {}: {}",
                manifest_path.display(),
                e
            ))
        })?;
        verify_commit_manifest_content(&manifest, &manifest_path)?;
        Ok(Some(manifest))
    }

    pub(crate) fn write_commit_manifest<P: AsRef<Path>>(
        &self,
        path: P,
        epoch: u64,
        kind: CommitKind,
        base_epoch: Option<u64>,
    ) -> StorageResult<()> {
        let files = collect_committed_files(path.as_ref())?;
        // Sidecars flush before this call (full) or already sit beside the
        // baseline (incremental); inventory runs last so the pin only names
        // sidecars that exist on disk at the commit point.
        let sidecars = collect_sidecar_records(path.as_ref());
        let format_version = MANIFEST_FORMAT_VERSION;
        let written_at_ms = now_ms();
        let checksum = commit_manifest_checksum(CommitManifestInput {
            format_version,
            epoch,
            kind,
            base_epoch,
            generation: self.generation,
            files: &files,
            sidecars: &sidecars,
            written_at_ms,
        });
        let manifest = CommitManifest {
            format_version,
            epoch,
            kind,
            base_epoch,
            generation: self.generation,
            files,
            sidecars: sidecars.clone(),
            written_at_ms,
            checksum,
        };
        let payload = serde_json::to_vec(&manifest)
            .map_err(|e| graphdb_core::StorageError::serialize_error(e.to_string()))?;
        crate::compression::write_shadow_file(commit_manifest_path(path.as_ref()), &payload)?;
        // Expired sidecars (dropped columns, fully resident tables) leave
        // only after the new manifest commits, so a crash never orphans a
        // sidecar the previous manifest still pins.
        sweep_unpinned_sidecars(path.as_ref(), &sidecars);
        Ok(())
    }

    pub(crate) fn verify_commit_manifest(
        &self,
        path: &Path,
        manifest: &CommitManifest,
    ) -> StorageResult<()> {
        if manifest.generation != self.generation {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "checkpoint epoch {} kind={} belongs to redistribution generation {} but the \
                 table opens generation {}: refusing a checkpoint mixed in from another \
                 lineage instead of mis-decoding global IDs",
                manifest.epoch,
                manifest.kind.as_str(),
                manifest.generation,
                self.generation,
            )));
        }
        for rel in &manifest.files {
            let full = path.join(rel);
            if !full.exists() {
                if let Some(col) = column_for_isolatable_file(rel) {
                    log::warn!(
                        "checkpoint epoch {} kind={} column file missing (isolatable): file={} column={}: \
                         opening with that column unavailable instead of refusing the table",
                        manifest.epoch,
                        manifest.kind.as_str(),
                        full.display(),
                        col,
                    );
                    continue;
                }
                return Err(graphdb_core::StorageError::deserialize_error(format!(
                    "class={} checkpoint epoch {} kind={} incomplete: manifest-listed file missing: file={}",
                    CorruptionClass::Fatal.as_str(),
                    manifest.epoch,
                    manifest.kind.as_str(),
                    full.display(),
                )));
            }
        }
        Ok(())
    }
}
