//! Vertex table crash-recovery contract.
//!
//! Durability spans two layers with exactly one commit point:
//!
//! 1. Commit: the engine transaction WAL (`InsertVertexRedo` and peers)
//!    is the durable source of truth for committed but unflushed rows.
//!    Crash recovery replays it before tables serve reads, so a crash
//!    between commit and flush loses nothing as long as replay runs.
//! 2. Checkpoint: full or incremental flush writes shard files first and
//!    `commit_manifest.json` last. The manifest lists every file the open
//!    path may trust; files outside it are never read.
//! 3. Open: a manifest-listed file that is missing or corrupt refuses the
//!    whole table open instead of running sick. There is deliberately no
//!    degraded single-shard open: serving a subset of shards would hand
//!    out global IDs whose siblings silently vanished, which readers
//!    cannot distinguish from genuine absence.
//!
//! Fault-injection coverage for this contract lives in
//! `crates/graphdb-storage/tests/persistence_recovery.rs`: flush plus
//! reload, plus corrupt-manifest refusal (a manifest-listed file that is
//! missing or corrupt must fail the open).

use std::path::{Path, PathBuf};

use super::ShardedVertexTable;
use crate::compression::CompressionType;
use graphdb_core::StorageResult;

/// Table-level manifest file pinning the shard layout that global internal
/// IDs were encoded with. Global IDs embed the shard count in their bit
/// layout, so opening a table persisted with a different shard count would
/// silently mis-decode every ID. The manifest makes that a loud error.
const TABLE_MANIFEST_FILE_NAME: &str = "table_manifest.json";

/// Commit manifest pinning one checkpoint of this table. Written atomically
/// after all shard files, it is the only commit point the recovery path
/// trusts: files outside the manifest are never read, and a manifest-listed
/// file that is missing or corrupt refuses the open instead of running sick.
pub(crate) const COMMIT_MANIFEST_FILE_NAME: &str = "commit_manifest.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct TableManifest {
    format_version: u8,
    label: graphdb_core::types::LabelId,
    label_name: String,
    num_shards: usize,
    segment_slots_bits: u32,
    total_segments: u32,
    /// Routing scheme version that encoded the persisted global IDs (see
    /// `ROUTER_VERSION`). Decoding with any other scheme would misroute
    /// every row, so unknown versions refuse the open.
    router_version: u8,
    /// Redistribution generation of this table lineage. Fresh tables start
    /// at zero; each offline redistribution bumps it. The commit manifest
    /// carries the same number so the open path can refuse a checkpoint
    /// mixed in from another generation instead of mis-decoding it.
    generation: u64,
    /// Wall-clock milliseconds of the last full baseline flush, written on
    /// every full flush and preserved across incremental flushes. Drives
    /// the baseline-age signal across restarts; missing (old manifests)
    /// never refuses the open, only warns and falls back to the in-process
    /// estimate.
    #[serde(default)]
    last_full_flush_ms: Option<u64>,
    checksum: u32,
}

/// Persistent layout version of both manifests. Stays at 1 through the
/// development phase: any manifest whose version differs from this constant
/// is rejected with a rebuild directive; there is no automatic migration
/// and old on-disk data is never made compatible.
const MANIFEST_FORMAT_VERSION: u8 = 1;

struct TableManifestInput<'a> {
    format_version: u8,
    label: graphdb_core::types::LabelId,
    label_name: &'a str,
    num_shards: usize,
    segment_slots_bits: u32,
    total_segments: u32,
    router_version: u8,
    generation: u64,
    last_full_flush_ms: Option<u64>,
}

fn table_manifest_checksum(input: TableManifestInput<'_>) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[input.format_version]);
    hasher.update(&input.label.to_le_bytes());
    hasher.update(input.label_name.as_bytes());
    hasher.update(&(input.num_shards as u64).to_le_bytes());
    hasher.update(&input.segment_slots_bits.to_le_bytes());
    hasher.update(&input.total_segments.to_le_bytes());
    hasher.update(&[input.router_version]);
    hasher.update(&input.generation.to_le_bytes());
    // Absent timestamps hash as zero so a fresh table and an old manifest
    // share one checksum shape; old manifests without the field fall back
    // to the legacy checksum in verification instead of refusing the open.
    hasher.update(&input.last_full_flush_ms.unwrap_or(0).to_le_bytes());
    hasher.finalize()
}

fn legacy_table_manifest_checksum(input: TableManifestInput<'_>) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[input.format_version]);
    hasher.update(&input.label.to_le_bytes());
    hasher.update(input.label_name.as_bytes());
    hasher.update(&(input.num_shards as u64).to_le_bytes());
    hasher.update(&input.segment_slots_bits.to_le_bytes());
    hasher.update(&input.total_segments.to_le_bytes());
    hasher.update(&[input.router_version]);
    hasher.update(&input.generation.to_le_bytes());
    hasher.finalize()
}

fn commit_manifest_checksum(
    format_version: u8,
    epoch: u64,
    kind: CommitKind,
    base_epoch: Option<u64>,
    generation: u64,
    files: &[String],
    sidecars: &[SnapshotSidecarRecord],
    written_at_ms: u64,
) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[format_version]);
    hasher.update(&epoch.to_le_bytes());
    hasher.update(kind.as_str().as_bytes());
    hasher.update(&base_epoch.unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(&generation.to_le_bytes());
    for file in files {
        hasher.update(file.as_bytes());
        hasher.update(&[0]);
    }
    for sidecar in sidecars {
        hasher.update(sidecar.file.as_bytes());
        hasher.update(&[0]);
        hasher.update(&sidecar.bytes.to_le_bytes());
        hasher.update(&sidecar.checksum.to_le_bytes());
    }
    hasher.update(&written_at_ms.to_le_bytes());
    hasher.finalize()
}

fn verify_table_manifest(manifest: &TableManifest, path: &Path) -> StorageResult<()> {
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
        // Old manifests predate the baseline-timestamp field and decode it
        // as missing: accept them through the legacy checksum instead of
        // refusing the open. The load path warns and falls back to the
        // in-process age estimate.
        let legacy = legacy_table_manifest_checksum(TableManifestInput {
            format_version: manifest.format_version,
            label: manifest.label,
            label_name: &manifest.label_name,
            num_shards: manifest.num_shards,
            segment_slots_bits: manifest.segment_slots_bits,
            total_segments: manifest.total_segments,
            router_version: manifest.router_version,
            generation: manifest.generation,
            last_full_flush_ms: None,
        });
        if manifest.last_full_flush_ms.is_some() || legacy != manifest.checksum {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "table manifest checksum mismatch at {}: expected {:#010x}, got {:#010x}",
                path.display(),
                expected,
                manifest.checksum,
            )));
        }
    }
    let layout = super::routing::ShardLayout {
        num_shards: manifest.num_shards,
        segment_slots_bits: manifest.segment_slots_bits,
        total_segments: manifest.total_segments,
    };
    if manifest.router_version != super::routing::ROUTER_VERSION {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "unsupported table router version {} at {}, expected {}: \
             the external-key routing scheme changed; rebuild the table with the \
             offline redistribution tool instead of opening it in place",
            manifest.router_version,
            path.display(),
            super::routing::ROUTER_VERSION,
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

fn verify_commit_manifest_content(manifest: &CommitManifest, path: &Path) -> StorageResult<()> {
    if manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "unsupported commit manifest version {} at {}, expected {}",
            manifest.format_version,
            path.display(),
            MANIFEST_FORMAT_VERSION,
        )));
    }
    let expected = commit_manifest_checksum(
        manifest.format_version,
        manifest.epoch,
        manifest.kind,
        manifest.base_epoch,
        manifest.generation,
        &manifest.files,
        &manifest.sidecars,
        manifest.written_at_ms,
    );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CommitKind {
    Full,
    Incremental,
}

impl CommitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

/// One checkpoint sidecar pinned in the commit manifest: a derived
/// `{column}.snapshot` eviction cache beside the authoritative pages.
/// Sidecars are verifiable but discardable: a missing or corrupt sidecar
/// keeps chunks resident and never refuses the open, while the manifest
/// pin lets reload tell a pruned sidecar from a tampered one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SnapshotSidecarRecord {
    /// Manifest-relative path (`shard_0/name.snapshot`).
    pub(crate) file: String,
    /// File bytes at flush time.
    pub(crate) bytes: u64,
    /// CRC32 over the file bytes at flush time.
    pub(crate) checksum: u32,
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
    /// Derived eviction sidecars pinned for verification only. Absent in
    /// old manifests (decoded as empty); never part of the strict file
    /// set, so a missing or corrupt sidecar never refuses the open.
    #[serde(default)]
    pub(crate) sidecars: Vec<SnapshotSidecarRecord>,
    pub(crate) written_at_ms: u64,
    pub(crate) checksum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHealthReport {
    /// Whether `commit_manifest.json` exists.
    pub manifest_present: bool,
    /// Whether the manifest decoded as JSON.
    pub manifest_decodable: bool,
    /// Checkpoint epoch pinned by the manifest, if decodable.
    pub epoch: Option<u64>,
    /// `full` or `incremental`, if decodable.
    pub kind: Option<String>,
    /// Base epoch for incremental checkpoints, if decodable.
    pub base_epoch: Option<u64>,
    /// Redistribution generation pinned by the commit manifest, if decodable.
    pub commit_generation: Option<u64>,
    /// Routing scheme version pinned by the table manifest, if decodable.
    pub router_version: Option<u8>,
    /// Redistribution generation pinned by the table manifest, if decodable.
    pub generation: Option<u64>,
    /// Lineage defects: unknown router, table/commit generation mismatch,
    /// or an undecodable table manifest. Empty when the lineage proves.
    pub lineage_issues: Vec<String>,
    /// Files listed by the manifest.
    pub listed_files: Vec<String>,
    /// Listed files missing from disk.
    pub missing_files: Vec<String>,
    /// Orphan temp/staging files (tolerated, cleaned by recovery).
    pub orphan_tmp_files: Vec<String>,
    /// Whether every shard's primary-key files decode and agree.
    pub pk_index_ok: bool,
    /// Per-shard primary-key decode issues, empty when healthy.
    pub pk_issues: Vec<String>,
    /// Sidecars pinned by the manifest.
    pub sidecars: Vec<String>,
    /// Discardable sidecar defects (missing, checksum mismatch, or corrupt
    /// frames). Never block `is_healthy`: reload drops the sidecar and
    /// keeps the chunks resident.
    pub sidecar_issues: Vec<String>,
}

impl CommitHealthReport {
    /// Whether the directory is safe to open strictly: a decodable manifest
    /// with no missing files, a verifiable primary-key index, and a proven
    /// lineage (known router, matching table and commit generations).
    /// Sidecar defects never block health: they are discardable caches.
    pub fn is_healthy(&self) -> bool {
        self.manifest_present
            && self.manifest_decodable
            && self.missing_files.is_empty()
            && self.pk_index_ok
            && self.lineage_issues.is_empty()
    }

    /// Discardable sidecar defects observable without failing the open.
    pub fn sidecar_discards(&self) -> usize {
        self.sidecar_issues.len()
    }
}

/// Aggregated health across label directories under one vertices root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalCommitHealth {
    /// Per-table reports keyed by directory name.
    pub tables: Vec<(String, CommitHealthReport)>,
    /// Whether the baseline plus incremental epoch chain is continuous.
    pub chain_ok: bool,
    /// Human-readable issues, empty when healthy.
    pub issues: Vec<String>,
}

/// Damage classification for the graded open path. Fatal defects refuse
/// every open mode (table manifest, commit manifest, lineage); isolatable
/// defects refuse the strict open but load healthy shards in repair mode.
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

/// One shard-level damage record for offline repair tooling. Carries the
/// machine-readable classification plus file location so scripts parse
/// fields instead of matching log text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShardDamage {
    pub shard: usize,
    pub file: String,
    pub class: CorruptionClass,
    pub reason: String,
}

/// Offline repair-mode open report. Healthy shards are loaded and
/// diagnosable; damaged shards are skipped. The handle is diagnostic
/// read-only: callers must not serve writes from a partially opened table.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepairReport {
    pub healthy_shards: Vec<usize>,
    pub damaged_shards: Vec<ShardDamage>,
    /// Epoch pinned by the manifest when lineage proved, if any.
    pub epoch: Option<u64>,
}

impl RepairReport {
    pub fn is_complete(&self) -> bool {
        self.damaged_shards.is_empty()
    }
}

impl GlobalCommitHealth {
    /// Whether every table is healthy and the epoch chain is continuous.
    pub fn is_healthy(&self) -> bool {
        self.chain_ok && self.tables.iter().all(|(_, r)| r.is_healthy())
    }
}

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn commit_manifest_path(dir: &Path) -> PathBuf {
    dir.join(COMMIT_MANIFEST_FILE_NAME)
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

/// CRC32 over one sidecar file's bytes, with its length. `None` when the
/// file cannot be read; the caller records no pin instead of failing the
/// checkpoint over a derived cache.
fn sidecar_file_fingerprint(path: &Path) -> Option<(u64, u32)> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes);
    Some((bytes.len() as u64, hasher.finalize()))
}

/// Inventory every `{column}.snapshot` sidecar under the table directory
/// as manifest-relative records. Runs after the per-shard sidecar flush
/// and before the commit manifest write, so the manifest always points at
/// sidecars that exist; post-manifest orphan sweep drops only unpinned
/// sidecars. Unreadable sidecars are skipped (derived cache, never fatal).
fn collect_sidecar_records(dir: &Path) -> Vec<SnapshotSidecarRecord> {
    let mut out = Vec::new();
    for index in 0..usize::MAX {
        let shard_dir = dir.join(format!("shard_{}", index));
        if !shard_dir.exists() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&shard_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !name.ends_with(".snapshot") {
                continue;
            }
            let Some((bytes, checksum)) = sidecar_file_fingerprint(&path) else {
                continue;
            };
            out.push(SnapshotSidecarRecord {
                file: format!("shard_{}/{}", index, name),
                bytes,
                checksum,
            });
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// Remove sidecar files that the committed manifest does not pin. Runs
/// after the manifest commit so a crash between sidecar flush and manifest
/// write never deletes a sidecar the previous manifest still pins; only
/// sidecars absent from the new manifest (dropped columns, fully resident
/// tables) are swept. Tolerant: delete failures only warn.
fn sweep_unpinned_sidecars(dir: &Path, pinned: &[SnapshotSidecarRecord]) {
    use std::collections::HashSet;
    let live: HashSet<&str> = pinned.iter().map(|r| r.file.as_str()).collect();
    for index in 0..usize::MAX {
        let shard_dir = dir.join(format!("shard_{}", index));
        if !shard_dir.exists() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&shard_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !name.ends_with(".snapshot") {
                continue;
            }
            let rel = format!("shard_{}/{}", index, name);
            if live.contains(rel.as_str()) {
                continue;
            }
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!(
                    "orphan sidecar cleanup: cannot remove {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// Remove staging and shadow leftovers. Tolerant by design: a failed delete
/// only warns, never fails the checkpoint or the open.
fn cleanup_orphans_tolerant(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let is_staging = name.ends_with(".staging") || name.ends_with(".tmp") || name == "staging";
        if !is_staging {
            continue;
        }
        let res = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = res {
            log::warn!("orphan cleanup: cannot remove {}: {}", path.display(), e);
        }
    }
    for i in 0..usize::MAX {
        let shard_dir = dir.join(format!("shard_{}", i));
        if !shard_dir.exists() {
            break;
        }
        if let Err(e) = crate::compression::cleanup_shadow_files(&shard_dir) {
            log::warn!(
                "orphan cleanup: cannot clear shadow files in {}: {}",
                shard_dir.display(),
                e
            );
        }
    }
    if let Err(e) = crate::compression::cleanup_shadow_files(dir) {
        log::warn!(
            "orphan cleanup: cannot clear shadow files in {}: {}",
            dir.display(),
            e
        );
    }
}

/// Manifest-pinned identity of one persisted table lineage: the shard
/// layout global IDs decode with, the routing scheme version that placed
/// them, and the redistribution generation they belong to. The open path
/// adopts all three; anything less would mis-decode or mix generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManifestLineage {
    pub(crate) layout: super::routing::ShardLayout,
    pub(crate) router_version: u8,
    pub(crate) generation: u64,
}

/// Staging receipt for one offline redistribution, written beside the
/// rebuilt checkpoint it describes. Binds the source and target lineage
/// generations so the adopt step can prove continuity (target is exactly
/// source plus one) instead of trusting directory placement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
// Staging/adopt protocol primitive: the online adopt driver consumes the
// receipt before swapping generations. Retained as the reuse target for
// that driver; exercised by the staging tests below.
#[allow(dead_code)]
pub(crate) struct RedistributionReceipt {
    pub(crate) source_generation: u64,
    pub(crate) target_generation: u64,
    pub(crate) source_shards: usize,
    pub(crate) target_shards: usize,
    pub(crate) rows: usize,
    pub(crate) mappings: usize,
}

/// Receipt file pinning the lineage handoff of a staged redistribution.
#[allow(dead_code)]
const RESHARD_RECEIPT_FILE_NAME: &str = "reshard_receipt.json";

impl ShardedVertexTable {
    /// Rebuild this table under `new_num_shards` into a staging directory.
    ///
    /// Runs the fenced [`ShardedVertexTable::reshard_to`] rebuild, flushes
    /// the new generation as a full checkpoint into `staging`, and writes a
    /// receipt binding the source and target generations. The caller swaps
    /// the staging directory into place (online protocol) or retires the
    /// old directory (offline runbook) only after
    /// [`Self::check_staged_redistribution`] proves lineage continuity.
    /// The source table and its directory stay untouched.
    // See the receipt-type note: staging entry point for the adopt driver.
    #[allow(dead_code)]
    pub fn redistribute_to_staging<P: AsRef<Path>>(
        &self,
        staging: P,
        new_num_shards: usize,
        compression: CompressionType,
    ) -> StorageResult<RedistributionReceipt> {
        let (rebuilt, mapping) = self.reshard_to(new_num_shards)?;
        rebuilt.flush(&staging, compression)?;
        let receipt = RedistributionReceipt {
            source_generation: self.generation,
            target_generation: rebuilt.generation,
            source_shards: self.layout.num_shards,
            target_shards: rebuilt.layout.num_shards,
            rows: rebuilt.approximate_total_count(),
            mappings: mapping.len(),
        };
        let payload = serde_json::to_vec(&receipt)
            .map_err(|e| graphdb_core::StorageError::serialize_error(e.to_string()))?;
        crate::compression::write_shadow_file(
            staging.as_ref().join(RESHARD_RECEIPT_FILE_NAME),
            &payload,
        )?;
        Ok(receipt)
    }

    /// Prove a staged redistribution is safe to adopt for a source table at
    /// `source_generation`: the receipt must exist and decode, its source
    /// must be the caller, its target must be exactly source plus one, and
    /// the staged table manifest must pin that same target generation.
    /// Anything else refuses the adopt instead of swapping in a foreign
    /// checkpoint.
    // See the receipt-type note: adoption gate for the adopt driver.
    #[allow(dead_code)]
    pub fn check_staged_redistribution<P: AsRef<Path>>(
        staging: P,
        source_generation: u64,
    ) -> StorageResult<RedistributionReceipt> {
        let staging = staging.as_ref();
        let receipt_path = staging.join(RESHARD_RECEIPT_FILE_NAME);
        let payload = std::fs::read(&receipt_path).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "staged redistribution at {} has no readable receipt {}: {e}",
                staging.display(),
                receipt_path.display(),
            ))
        })?;
        let receipt: RedistributionReceipt = serde_json::from_slice(&payload).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "invalid redistribution receipt {}: {e}",
                receipt_path.display(),
            ))
        })?;
        if receipt.source_generation != source_generation
            || receipt.target_generation != source_generation.saturating_add(1)
        {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "staged redistribution at {} breaks lineage continuity: receipt hands \
                 generation {} to {}, but the source table is at generation {}",
                staging.display(),
                receipt.source_generation,
                receipt.target_generation,
                source_generation,
            )));
        }
        let lineage = Self::manifest_layout(staging)?.ok_or_else(|| {
            graphdb_core::StorageError::deserialize_error(format!(
                "staged redistribution at {} is missing its table manifest",
                staging.display(),
            ))
        })?;
        if lineage.generation != receipt.target_generation {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "staged redistribution at {} mixes generations: receipt promises {} \
                 but the staged manifest pins {}",
                staging.display(),
                receipt.target_generation,
                lineage.generation,
            )));
        }
        // Continuity alone does not prove the staged files decode: refuse
        // the adopt when the staged checkpoint itself is unhealthy so a
        // half-written or tampered staging never swaps into place.
        let report = Self::inspect_commit_health(staging).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "staged redistribution at {} failed health inspection: {e}",
                staging.display(),
            ))
        })?;
        if !report.is_healthy() {
            let mut defects = report.lineage_issues.clone();
            if !report.manifest_present {
                defects.push("commit manifest missing".to_string());
            } else if !report.manifest_decodable {
                defects.push("commit manifest undecodable".to_string());
            }
            for missing in &report.missing_files {
                defects.push(format!("listed file missing: {missing}"));
            }
            defects.extend(report.pk_issues.clone());
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "staged redistribution at {} unhealthy, refusing adopt: {}",
                staging.display(),
                defects.join("; "),
            )));
        }
        Ok(receipt)
    }
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

    fn write_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let format_version = MANIFEST_FORMAT_VERSION;
        let last_full_flush_ms = self.persisted_baseline_ms();
        let checksum = table_manifest_checksum(TableManifestInput {
            format_version,
            label: self.label,
            label_name: &self.label_name,
            num_shards: self.layout.num_shards,
            segment_slots_bits: self.layout.segment_slots_bits,
            total_segments: self.layout.total_segments,
            router_version: super::routing::ROUTER_VERSION,
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
            router_version: super::routing::ROUTER_VERSION,
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
            layout: super::routing::ShardLayout {
                num_shards: manifest.num_shards,
                segment_slots_bits: manifest.segment_slots_bits,
                total_segments: manifest.total_segments,
            },
            router_version: manifest.router_version,
            generation: manifest.generation,
        }))
    }

    fn read_table_manifest<P: AsRef<Path>>(path: P) -> StorageResult<Option<TableManifest>> {
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

    fn write_commit_manifest<P: AsRef<Path>>(
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
        let checksum = commit_manifest_checksum(
            format_version,
            epoch,
            kind,
            base_epoch,
            self.generation,
            &files,
            &sidecars,
            written_at_ms,
        );
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

    /// Delete temp/staging leftovers without failing. Orphan files and
    /// manifest-external files are always tolerated; only manifest-listed
    /// content is strict.
    pub fn cleanup_orphans<P: AsRef<Path>>(path: P) {
        cleanup_orphans_tolerant(path.as_ref());
    }

    /// Offline read-only health inspection for one table directory.
    ///
    /// Reuses the recovery path's manifest decoding plus file existence
    /// checks and reports: whether the commit manifest is present and
    /// decodable, its epoch/kind/base-epoch chain pointers, which listed
    /// files are missing, which orphan temp files exist, and whether every
    /// shard's primary-key files decode and agree. Never writes;
    /// cleanup stays with startup recovery. Baseline plus incremental epoch
    /// chain continuity across directories is validated by the global
    /// checkpoint manifest manager, which sees every table's pointers.
    pub fn inspect_commit_health<P: AsRef<Path>>(path: P) -> StorageResult<CommitHealthReport> {
        let dir = path.as_ref();
        let manifest_path = commit_manifest_path(dir);
        let manifest_present = manifest_path.exists();
        let mut manifest_decodable = false;
        let mut epoch = None;
        let mut kind = None;
        let mut base_epoch = None;
        let mut commit_generation = None;
        let mut listed_files = Vec::new();
        let mut missing_files = Vec::new();
        let mut pinned_sidecars: Vec<SnapshotSidecarRecord> = Vec::new();
        if manifest_present {
            if let Ok(payload) = std::fs::read(&manifest_path) {
                if let Ok(manifest) = serde_json::from_slice::<CommitManifest>(&payload) {
                    if verify_commit_manifest_content(&manifest, &manifest_path).is_ok() {
                        manifest_decodable = true;
                        epoch = Some(manifest.epoch);
                        kind = Some(manifest.kind.as_str().to_string());
                        base_epoch = manifest.base_epoch;
                        commit_generation = Some(manifest.generation);
                        listed_files = manifest.files.clone();
                        for rel in &manifest.files {
                            if !dir.join(rel).exists() {
                                missing_files.push(rel.clone());
                            }
                        }
                        pinned_sidecars = manifest.sidecars.clone();
                    }
                }
            }
        }
        let mut router_version = None;
        let mut generation = None;
        let mut lineage_issues = Vec::new();
        match Self::read_table_manifest(dir) {
            Ok(Some(table)) => {
                router_version = Some(table.router_version);
                generation = Some(table.generation);
                if table.router_version != super::routing::ROUTER_VERSION {
                    lineage_issues.push(format!(
                        "unknown table router version {} (expected {})",
                        table.router_version,
                        super::routing::ROUTER_VERSION,
                    ));
                }
                match commit_generation {
                    Some(commit) if commit != table.generation => lineage_issues.push(format!(
                        "table manifest generation {} differs from commit generation {}",
                        table.generation, commit,
                    )),
                    None if manifest_present => lineage_issues.push(
                        "commit manifest undecodable: checkpoint lineage unprovable".to_string(),
                    ),
                    _ => {}
                }
            }
            Ok(None) => lineage_issues.push(format!(
                "table manifest missing at {}: refusing open without shard layout pin; \
                 rebuild the table with the offline redistribution tool",
                dir.join(TABLE_MANIFEST_FILE_NAME).display(),
            )),
            // The strict decode error already names the file and the rebuild
            // directive (bad JSON, checksum, version, router, or layout), so
            // forwarding it keeps diagnosis and refusal in agreement.
            Err(e) => lineage_issues.push(format!("table manifest rejected: {e}")),
        }
        let mut orphan_tmp_files = Vec::new();
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".tmp") || name.ends_with(".staging") || name == "staging" {
                        orphan_tmp_files.push(name);
                    }
                }
            }
            for index in 0..usize::MAX {
                let shard_dir = dir.join(format!("shard_{}", index));
                if !shard_dir.exists() {
                    break;
                }
                if let Ok(entries) = std::fs::read_dir(&shard_dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".tmp") {
                            orphan_tmp_files.push(format!("shard_{}/{}", index, name));
                        }
                    }
                }
            }
        }
        orphan_tmp_files.sort();
        let mut pk_issues = Vec::new();
        for index in 0..usize::MAX {
            let shard_dir = dir.join(format!("shard_{}", index));
            if !shard_dir.exists() {
                break;
            }
            for issue in crate::vertex::vertex_table::core::VertexTable::verify_pk_files(&shard_dir)
            {
                pk_issues.push(format!("shard_{}: {}", index, issue));
            }
        }
        pk_issues.sort();
        let pk_index_ok = pk_issues.is_empty();
        // Sidecar verification is discardable-only: a missing, checksum
        // mismatched, or unparseable sidecar is reported but never blocks
        // health. Reload drops that sidecar and keeps chunks resident.
        let mut sidecars: Vec<String> = pinned_sidecars.iter().map(|r| r.file.clone()).collect();
        sidecars.sort();
        let mut sidecar_issues = Vec::new();
        for record in &pinned_sidecars {
            let full = dir.join(&record.file);
            let bytes = std::fs::read(&full);
            match bytes {
                Err(_) => {
                    sidecar_issues.push(format!("sidecar missing (discardable): {}", record.file))
                }
                Ok(payload) => {
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&payload);
                    if payload.len() as u64 != record.bytes || hasher.finalize() != record.checksum
                    {
                        sidecar_issues.push(format!(
                            "sidecar checksum mismatch (discardable): {}",
                            record.file
                        ));
                        continue;
                    }
                    if crate::vertex::column::chunk_residency::open_snapshot_sidecar(&full).is_err()
                    {
                        sidecar_issues
                            .push(format!("sidecar corrupt (discardable): {}", record.file));
                    }
                }
            }
        }
        // Unpinned sidecars on disk (crash window or dropped columns) are
        // swept on the next checkpoint; report but never fail health.
        for index in 0..usize::MAX {
            let shard_dir = dir.join(format!("shard_{}", index));
            if !shard_dir.exists() {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&shard_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".snapshot") {
                    continue;
                }
                let rel = format!("shard_{}/{}", index, name);
                if !sidecars.contains(&rel) {
                    sidecar_issues.push(format!("sidecar unpinned (swept on flush): {}", rel));
                }
            }
        }
        sidecar_issues.sort();
        Ok(CommitHealthReport {
            manifest_present,
            manifest_decodable,
            epoch,
            kind,
            base_epoch,
            commit_generation,
            router_version,
            generation,
            lineage_issues,
            listed_files,
            missing_files,
            orphan_tmp_files,
            pk_index_ok,
            pk_issues,
            sidecars,
            sidecar_issues,
        })
    }

    /// Offline read-only inspection across label directories.
    ///
    /// Walks `vertices_dir` for `label_*` subdirectories, reuses the
    /// table-level inspection per label, and validates baseline plus
    /// incremental epoch chain continuity: every incremental base epoch must
    /// appear as another table epoch or as a full checkpoint epoch in the
    /// same walk. Never writes; cleanup stays with startup recovery. Serves
    /// as the offline patrol entry for half-damaged stores.
    pub fn inspect_store_health<P: AsRef<Path>>(
        vertices_dir: P,
    ) -> StorageResult<GlobalCommitHealth> {
        let root = vertices_dir.as_ref();
        let mut tables = Vec::new();
        let mut issues = Vec::new();
        if root.exists() {
            let mut names: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("label_") {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
            names.sort();
            for name in names {
                match Self::inspect_commit_health(root.join(&name)) {
                    Ok(report) => {
                        if !report.manifest_present {
                            issues.push(format!("{}: commit manifest missing", name));
                        } else if !report.manifest_decodable {
                            issues.push(format!("{}: commit manifest undecodable", name));
                        }
                        for missing in &report.missing_files {
                            issues.push(format!("{}: listed file missing: {}", name, missing));
                        }
                        for lineage in &report.lineage_issues {
                            issues.push(format!("{}: {}", name, lineage));
                        }
                        for pk_issue in &report.pk_issues {
                            issues.push(format!("{}: {}", name, pk_issue));
                        }
                        tables.push((name, report));
                    }
                    Err(e) => {
                        issues.push(format!("{}: inspection failed: {}", name, e));
                    }
                }
            }
        }
        let mut epochs: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (_, report) in &tables {
            if let Some(epoch) = report.epoch {
                epochs.insert(epoch);
            }
            if let Some(base) = report.base_epoch {
                epochs.insert(base);
            }
        }
        let mut chain_ok = true;
        for (name, report) in &tables {
            if report.kind.as_deref() == Some("incremental") {
                match (report.epoch, report.base_epoch) {
                    (Some(epoch), Some(base)) => {
                        if base >= epoch {
                            chain_ok = false;
                            issues.push(format!(
                                "{}: incremental base {} not older than epoch {}",
                                name, base, epoch
                            ));
                        }
                    }
                    _ => {
                        chain_ok = false;
                        issues.push(format!(
                            "{}: incremental checkpoint missing epoch pointers",
                            name
                        ));
                    }
                }
            }
        }
        if tables.is_empty() {
            issues.push("no label directories found".to_string());
        }
        let chain_ok_final = tables.iter().all(|(_, r)| r.missing_files.is_empty())
            && tables.iter().all(|(_, r)| {
                if r.manifest_present {
                    r.manifest_decodable
                } else {
                    false
                }
            })
            && chain_ok;
        Ok(GlobalCommitHealth {
            tables,
            chain_ok: chain_ok_final,
            issues,
        })
    }

    fn check_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
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

    fn verify_commit_manifest(&self, path: &Path, manifest: &CommitManifest) -> StorageResult<()> {
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

    /// Test-only force-encode plus evict for one column across shards.
    /// Uses per-column shared guards (released between columns) so no
    /// mapped guard outlives its shard latch.
    #[cfg(test)]
    pub(crate) fn force_encode_evict_for_test(&self, column: &str) {
        for shard in &self.shards {
            let table = shard.read();
            table.columns.for_each_column(|col| {
                if col.name == column {
                    let _ = col
                        .apply_encoding_to_chunks(crate::encoding::EncodingType::Dictionary, 255);
                    let _ = col.evict_chunk(0);
                }
            });
        }
    }

    /// Verify pinned sidecars against disk and prune tampered ones before
    /// any shard loads. A missing sidecar needs no action (kept resident);
    /// a present sidecar whose bytes or checksum differ from the pin, or
    /// whose frames fail to parse, is deleted as a discardable cache and
    /// counted. Only `.snapshot` files are ever removed, never
    /// authoritative pages, and failures only warn. Returns pruned count.
    fn prune_tampered_sidecars(dir: &Path, manifest: &CommitManifest) -> usize {
        let mut pruned = 0usize;
        for record in &manifest.sidecars {
            let full = dir.join(&record.file);
            if !full.exists() {
                continue;
            }
            let valid = match std::fs::read(&full) {
                Ok(payload) => {
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&payload);
                    payload.len() as u64 == record.bytes
                        && hasher.finalize() == record.checksum
                        && crate::vertex::column::chunk_residency::open_snapshot_sidecar(&full)
                            .is_ok()
                }
                Err(_) => false,
            };
            if !valid {
                log::warn!(
                    "pruning tampered snapshot sidecar {}: discarding cache, keeping chunks resident",
                    full.display(),
                );
                if std::fs::remove_file(&full).is_ok() {
                    pruned += 1;
                } else {
                    log::warn!("cannot prune tampered snapshot sidecar {}", full.display(),);
                }
            }
        }
        pruned
    }

    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        self.flush_with_epoch(path, compression, 0, CommitKind::Full, None)
    }

    pub fn flush_with_epoch<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
        epoch: u64,
        kind: CommitKind,
        base_epoch: Option<u64>,
    ) -> StorageResult<()> {
        use rayon::prelude::*;
        use std::fs;
        use std::sync::atomic::Ordering;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        cleanup_orphans_tolerant(path);
        self.shards
            .par_iter()
            .enumerate()
            .try_for_each(|(i, shard)| {
                let shard_dir = path.join(format!("shard_{}", i));
                shard.write().flush(&shard_dir, compression)
            })?;
        // Full flushes pin this completion time in the table manifest so
        // the baseline-age signal survives restarts. Stored before the
        // manifest write so the manifest carries this flush, not the
        // previous one; the commit-point order (files first, manifest
        // last) is unchanged.
        if kind == CommitKind::Full {
            self.last_full_flush_ms.store(now_ms(), Ordering::Release);
        }
        self.write_table_manifest(path)?;
        self.write_commit_manifest(path, epoch, kind, base_epoch)?;
        Ok(())
    }

    pub fn flush_incremental_with_epoch<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
        epoch: u64,
        base_epoch: Option<u64>,
    ) -> StorageResult<()> {
        use rayon::prelude::*;
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        cleanup_orphans_tolerant(path);
        // Advisory trigger verdict with its reason code: explains whether
        // this incremental is routine or overdue for a full baseline. The
        // kind stays incremental here; upgrading to full is the checkpoint
        // coordinator's call because the epoch chain points at the base.
        let plan = super::super::flush_trigger::decide(self.flush_signals());
        log::debug!(
            "vertex table '{}' incremental flush trigger: kind={:?} reason={} merge_pages={}",
            self.label_name,
            plan.kind,
            plan.reason.as_str(),
            plan.merge_pages,
        );
        self.shards
            .par_iter()
            .enumerate()
            .try_for_each(|(i, shard)| {
                let shard_dir = path.join(format!("shard_{}", i));
                let mut table = shard.write();
                let dirty: Vec<crate::persistence::dirty_page::PageId> = table.dirty_pages();
                if dirty.is_empty() {
                    let _ = std::fs::create_dir_all(&shard_dir);
                    if table.total_count() == 0 {
                        return Ok(());
                    }
                    table.flush_incremental(&shard_dir, &[], compression)
                } else {
                    table.flush_incremental(&shard_dir, &dirty, compression)
                }
            })?;
        self.write_table_manifest(path)?;
        self.write_commit_manifest(path, epoch, CommitKind::Incremental, base_epoch)?;
        Ok(())
    }

    pub fn total_pages(&self) -> usize {
        self.shards
            .iter()
            .map(|s| {
                let t = s.read();
                let rc = t.columns.row_count();
                if rc == 0 {
                    0
                } else {
                    rc.div_ceil(crate::persistence::dirty_page::ROWS_PER_PAGE)
                }
            })
            .sum()
    }

    pub fn clear_dirty(&self) {
        for shard in &self.shards {
            shard.write().clear_dirty();
        }
    }

    pub fn total_dirty_pages(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.read().columns.total_dirty_pages())
            .sum()
    }

    pub fn collect_dirty_pages(&self) -> Vec<crate::persistence::dirty_page::PageId> {
        let mut out = Vec::new();
        for shard in &self.shards {
            out.extend(shard.read().dirty_pages());
        }
        out
    }

    pub fn load<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        // Offline pre-flight: reuse the read-only inspection for a
        // diagnostic line before the strict recovery path runs. Read-only;
        // cleanup stays with startup recovery.
        match Self::inspect_commit_health(path) {
            Ok(report) => log::debug!(
                "vertex table '{}' pre-load health: healthy={} epoch={:?} missing={} orphans={}",
                self.label_name,
                report.is_healthy(),
                report.epoch,
                report.missing_files.len(),
                report.orphan_tmp_files.len(),
            ),
            Err(e) => log::debug!(
                "vertex table '{}' pre-load inspection failed: {}",
                self.label_name,
                e
            ),
        }
        // Refuse to mis-decode: persisted global IDs embed the shard count.
        self.check_table_manifest(path)?;
        // Adopt the persisted baseline timestamp so the age signal survives
        // restarts. Missing timestamps only warn and keep the in-process
        // estimate; a persisted future value (clock skew) adopts the larger
        // of the two with a warning, never moves the signal backward.
        match Self::read_table_manifest(path)?.and_then(|m| m.last_full_flush_ms) {
            Some(persisted) => {
                use std::sync::atomic::Ordering;
                let now = now_ms();
                let adopted = persisted.max(now);
                if persisted > now {
                    log::warn!(
                        "vertex table '{}' baseline timestamp {} is ahead of the current clock {}; \
                         adopting the persisted value",
                        self.label_name,
                        persisted,
                        now,
                    );
                }
                if self.last_full_flush_ms.load(Ordering::Acquire) == 0 {
                    self.last_full_flush_ms.store(adopted, Ordering::Release);
                }
            }
            None => log::warn!(
                "vertex table '{}' manifest at {} has no baseline timestamp; \
                 falling back to the in-process age estimate",
                self.label_name,
                path.display(),
            ),
        }
        match Self::read_commit_manifest(path)? {
            Some(manifest) => {
                self.verify_commit_manifest(path, &manifest)?;
                // Manifest-pinned sidecar verification runs before any shard
                // loads: a missing sidecar is simply absent (kept resident),
                // a checksum-mismatched or unparseable sidecar is pruned as
                // a discardable cache so a tampered-but-valid sidecar can
                // never serve stale values. Pruning only deletes derived
                // `.snapshot` files, never authoritative pages, and never
                // fails the open.
                let pruned = Self::prune_tampered_sidecars(path, &manifest);
                if pruned > 0 {
                    log::warn!(
                        "vertex table '{}' discarded {} tampered snapshot sidecars; keeping chunks resident",
                        self.label_name,
                        pruned,
                    );
                }
                for (i, shard) in self.shards.iter().enumerate() {
                    let shard_dir = path.join(format!("shard_{}", i));
                    if !shard_dir.exists() {
                        return Err(graphdb_core::StorageError::deserialize_error(format!(
                            "class={} checkpoint epoch {} incomplete: shard directory missing: file={}",
                            CorruptionClass::Isolatable.as_str(),
                            manifest.epoch,
                            shard_dir.display(),
                        )));
                    }
                    let mut table = shard.write();
                    table.load(&shard_dir).map_err(|e| {
                        graphdb_core::StorageError::deserialize_error(format!(
                            "class={} checkpoint epoch {} shard {} corrupt at file={}: {}",
                            CorruptionClass::Isolatable.as_str(),
                            manifest.epoch,
                            i,
                            shard_dir.display(),
                            e
                        ))
                    })?;
                }
                Ok(())
            }
            None => Err(graphdb_core::StorageError::deserialize_error(format!(
                "class={} vertex table '{}' missing commit manifest at file={}: refusing open without checkpoint pin",
                CorruptionClass::Fatal.as_str(),
                self.label_name,
                path.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))),
        }
    }

    /// Offline repair-mode open: fatal defects (table/commit manifest,
    /// lineage) still refuse with `class=fatal`; per-shard data defects
    /// load healthy shards read-only and report damaged ones with
    /// `class=isolatable` plus file location for script parsing.
    ///
    /// Reuses the strict manifest decoding and per-shard `load` entry, not
    /// a separate parser. The returned handle holds only healthy shards;
    /// callers must treat it as diagnostic read-only and never serve
    /// writes from it. Sidecar defects never appear here: they are pruned
    /// as discardable caches on the strict path.
    pub fn load_for_repair<P: AsRef<Path>>(&self, path: P) -> StorageResult<RepairReport> {
        let dir = path.as_ref();
        self.check_table_manifest(path.as_ref()).map_err(|e| {
            graphdb_core::StorageError::deserialize_error(format!(
                "class={} {}",
                CorruptionClass::Fatal.as_str(),
                e.message()
            ))
        })?;
        let manifest = Self::read_commit_manifest(dir)?.ok_or_else(|| {
            graphdb_core::StorageError::deserialize_error(format!(
                "class={} vertex table '{}' missing commit manifest at file={}",
                CorruptionClass::Fatal.as_str(),
                self.label_name,
                dir.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))
        })?;
        // Repair mode proves lineage only (fatal); missing or corrupt
        // per-shard files become isolatable shard damages below instead of
        // refusing the whole open.
        if manifest.generation != self.generation {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "class={} checkpoint epoch {} kind={} belongs to redistribution generation {} but the table opens generation {}",
                CorruptionClass::Fatal.as_str(),
                manifest.epoch,
                manifest.kind.as_str(),
                manifest.generation,
                self.generation,
            )));
        }
        let _ = Self::prune_tampered_sidecars(dir, &manifest);
        let mut healthy_shards = Vec::new();
        let mut damaged_shards = Vec::new();
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = dir.join(format!("shard_{}", i));
            if !shard_dir.exists() {
                damaged_shards.push(ShardDamage {
                    shard: i,
                    file: shard_dir.display().to_string(),
                    class: CorruptionClass::Isolatable,
                    reason: format!(
                        "checkpoint epoch {} shard directory missing",
                        manifest.epoch
                    ),
                });
                continue;
            }
            match shard.write().load(&shard_dir) {
                Ok(()) => healthy_shards.push(i),
                Err(e) => damaged_shards.push(ShardDamage {
                    shard: i,
                    file: shard_dir.display().to_string(),
                    class: CorruptionClass::Isolatable,
                    reason: format!("checkpoint epoch {} shard corrupt: {}", manifest.epoch, e),
                }),
            }
        }
        Ok(RepairReport {
            healthy_shards,
            damaged_shards,
            epoch: Some(manifest.epoch),
        })
    }

    pub fn apply_delta_pages<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        match Self::read_commit_manifest(path)? {
            Some(manifest) => self.apply_delta_pages_strict(path, &manifest),
            None => Err(graphdb_core::StorageError::deserialize_error(format!(
                "missing commit manifest at {}: refusing delta apply without checkpoint pin",
                path.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))),
        }
    }

    fn apply_delta_pages_strict(
        &self,
        path: &Path,
        manifest: &CommitManifest,
    ) -> StorageResult<()> {
        self.verify_commit_manifest(path, manifest)?;
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            let has_delta = shard_dir.join("columns_pages").exists()
                || shard_dir.join("timestamps.bin").exists()
                || shard_dir.join("id_indexer.bin").exists()
                || shard_dir.join("id_indexer.delta").exists();
            if !(has_delta && shard_dir.exists()) {
                continue;
            }
            let mut table = shard.write();
            if shard_dir.join("columns_pages").exists() {
                table.apply_delta_pages(&shard_dir).map_err(|e| {
                    graphdb_core::StorageError::deserialize_error(format!(
                        "checkpoint epoch {} shard {} delta corrupt at {}: {}",
                        manifest.epoch,
                        i,
                        shard_dir.join("columns_pages").display(),
                        e
                    ))
                })?;
            }
            let ts_path = shard_dir.join("timestamps.bin");
            if ts_path.exists() {
                table.load_timestamps(&ts_path).map_err(|e| {
                    graphdb_core::StorageError::deserialize_error(format!(
                        "checkpoint epoch {} shard {} timestamps corrupt at {}: {}",
                        manifest.epoch,
                        i,
                        ts_path.display(),
                        e
                    ))
                })?;
            }
            Self::load_pk_overlay_strict(&mut table, &shard_dir, manifest, i)?;
        }
        Ok(())
    }

    /// Primary-key overlay for one shard: a full `id_indexer.bin` replaces
    /// the baseline (post-compaction anchor); otherwise `id_indexer.delta`
    /// applies onto the baseline state.
    fn load_pk_overlay_strict(
        table: &mut crate::vertex::vertex_table::core::VertexTable,
        shard_dir: &Path,
        manifest: &CommitManifest,
        shard_idx: usize,
    ) -> StorageResult<()> {
        let id_path = shard_dir.join("id_indexer.bin");
        if id_path.exists() {
            return table.load_id_indexer(&id_path).map_err(|e| {
                graphdb_core::StorageError::deserialize_error(format!(
                    "checkpoint epoch {} shard {} pk index corrupt at {}: {}",
                    manifest.epoch,
                    shard_idx,
                    id_path.display(),
                    e
                ))
            });
        }
        let delta_path = shard_dir.join("id_indexer.delta");
        if delta_path.exists() {
            table.load_id_indexer_delta(&delta_path).map_err(|e| {
                graphdb_core::StorageError::deserialize_error(format!(
                    "checkpoint epoch {} shard {} pk delta corrupt at {}: {}",
                    manifest.epoch,
                    shard_idx,
                    delta_path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod commit_tests {
    use super::*;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::Timestamp;
    use graphdb_core::{DataType, Value};

    fn test_schema() -> crate::vertex::VertexSchema {
        crate::vertex::VertexSchema {
            label_id: 1,
            label_name: "person".to_string(),
            properties: vec![StoragePropertyDef::new(
                "name".to_string(),
                DataType::String,
            )],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "commit_manifest_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn commit_manifest_pinned_and_strict_on_corrupt() {
        let dir = unique_dir("strict");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                7,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
        assert!(manifest_path.exists());
        let manifest = ShardedVertexTable::read_commit_manifest(&dir)
            .unwrap()
            .expect("commit manifest present");
        assert_eq!(manifest.epoch, 7);
        assert_eq!(manifest.kind, CommitKind::Full);
        assert!(!manifest.files.is_empty());

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v1", ts).is_some());

        let victim = dir.join("shard_0").join("columns.bin");
        if victim.exists() {
            std::fs::write(&victim, b"corrupt").unwrap();
            let err = reloaded.load(&dir).unwrap_err().to_string();
            assert!(
                err.contains('7') && err.contains("shard"),
                "strict error must carry epoch and shard location: {err}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_commit_manifest_refuses_open() {
        let dir = unique_dir("missing-manifest");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert(
                "v_missing",
                &[("name".to_string(), Value::from("v_missing"))],
                ts,
            )
            .unwrap();
        table
            .flush(&dir, CompressionType::Zstd { level: 0 })
            .unwrap();
        std::fs::remove_file(dir.join(COMMIT_MANIFEST_FILE_NAME)).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("commit manifest"),
            "missing manifest must refuse: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_tmp_cleaned_tolerantly() {
        let dir = unique_dir("orphan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stray.tmp"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("left.staging")).unwrap();
        ShardedVertexTable::cleanup_orphans(&dir);
        assert!(!dir.join("stray.tmp").exists());
        assert!(!dir.join("left.staging").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_inspection_reports_healthy_and_halfway_stores() {
        let dir = unique_dir("health");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                11,
                CommitKind::Full,
                None,
            )
            .unwrap();

        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.manifest_present && report.manifest_decodable);
        assert_eq!(report.epoch, Some(11));
        assert_eq!(report.kind.as_deref(), Some("full"));
        assert!(report.missing_files.is_empty());
        assert!(report.is_healthy());

        std::fs::write(dir.join("half.tmp"), b"x").unwrap();
        std::fs::write(dir.join("shard_0").join("page.tmp"), b"x").unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.is_healthy());
        assert_eq!(report.orphan_tmp_files.len(), 2);
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v1", ts).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fault_matrix_missing_listed_file_refuses_open() {
        let dir = unique_dir("fault-missing");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                13,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest = ShardedVertexTable::read_commit_manifest(&dir)
            .unwrap()
            .expect("manifest");
        let victim = manifest.files.first().expect("listed file").clone();
        std::fs::remove_file(dir.join(&victim)).unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(!report.is_healthy());
        assert_eq!(report.missing_files, vec![victim.clone()]);
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("13") && err.contains("missing"),
            "refusal must carry epoch and cause: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fault_matrix_broken_incremental_falls_back_to_baseline() {
        let base = unique_dir("fault-base");
        let incr = unique_dir("fault-incr");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &base,
                CompressionType::Zstd { level: 0 },
                21,
                CommitKind::Full,
                None,
            )
            .unwrap();
        table
            .insert("v2", &[("name".to_string(), Value::from("v2"))], ts)
            .unwrap();
        table
            .flush_incremental_with_epoch(&incr, CompressionType::Zstd { level: 0 }, 22, Some(21))
            .unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&incr).unwrap();
        assert_eq!(report.epoch, Some(22));
        assert_eq!(report.base_epoch, Some(21));

        for entry in std::fs::read_dir(&incr).unwrap().flatten() {
            let delta = entry.path().join("id_indexer.delta");
            if delta.exists() {
                std::fs::write(&delta, b"corrupt").unwrap();
                let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
                strict.load(&base).unwrap();
                let err = strict.apply_delta_pages(&incr).unwrap_err().to_string();
                assert!(err.contains("22"), "strict error carries epoch: {err}");
                break;
            }
        }
        let _ = std::fs::remove_file(incr.join(COMMIT_MANIFEST_FILE_NAME));
        let reloaded_missing =
            ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        reloaded_missing.load(&base).unwrap();
        assert!(reloaded_missing.apply_delta_pages(&incr).is_err());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
    }

    #[test]
    fn fault_matrix_corrupt_manifest_refuses_open() {
        let dir = unique_dir("fault-corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                17,
                CommitKind::Full,
                None,
            )
            .unwrap();
        std::fs::write(dir.join(COMMIT_MANIFEST_FILE_NAME), b"{broken").unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.manifest_present);
        assert!(!report.manifest_decodable);
        assert!(!report.is_healthy());
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("commit manifest"),
            "corrupt manifest must refuse with manifest cause: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fault_matrix_tampered_manifest_checksum_refuses_open() {
        let dir = unique_dir("fault-tamper");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                19,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["epoch"] = serde_json::Value::from(20u64);
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.manifest_present);
        assert!(!report.manifest_decodable);
        assert!(!report.is_healthy());
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("checksum"),
            "tampered manifest must refuse on checksum: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn table_manifest_pins_router_version_and_generation() {
        let dir = unique_dir("lineage-pinned");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                21,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let table_manifest = ShardedVertexTable::read_table_manifest(&dir)
            .unwrap()
            .expect("table manifest present");
        assert_eq!(
            table_manifest.router_version,
            super::super::routing::ROUTER_VERSION
        );
        assert_eq!(table_manifest.generation, 0);
        let commit_manifest = ShardedVertexTable::read_commit_manifest(&dir)
            .unwrap()
            .expect("commit manifest present");
        assert_eq!(
            commit_manifest.generation, table_manifest.generation,
            "commit and table manifests must pin the same lineage generation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_router_version_refuses_open() {
        let dir = unique_dir("lineage-router");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                23,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["router_version"] = serde_json::Value::from(99u64);
        manifest["checksum"] =
            serde_json::Value::from(table_manifest_checksum(TableManifestInput {
                format_version: manifest["format_version"].as_u64().unwrap() as u8,
                label: manifest["label"].as_u64().unwrap() as _,
                label_name: manifest["label_name"].as_str().unwrap(),
                num_shards: manifest["num_shards"].as_u64().unwrap() as usize,
                segment_slots_bits: manifest["segment_slots_bits"].as_u64().unwrap() as u32,
                total_segments: manifest["total_segments"].as_u64().unwrap() as u32,
                router_version: 99,
                generation: manifest["generation"].as_u64().unwrap(),
                last_full_flush_ms: manifest["last_full_flush_ms"].as_u64(),
            }));
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("router"),
            "unknown router version must refuse with router cause: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_generation_mismatch_refuses_open() {
        let dir = unique_dir("lineage-mismatch");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                25,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["generation"] = serde_json::Value::from(7u64);
        let kind = match manifest["kind"].as_str().unwrap() {
            "full" => CommitKind::Full,
            "incremental" => CommitKind::Incremental,
            other => panic!("unexpected commit kind {other}"),
        };
        let files: Vec<String> = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let sidecars: Vec<SnapshotSidecarRecord> =
            serde_json::from_value(manifest.get("sidecars").cloned().unwrap_or_default())
                .unwrap_or_default();
        manifest["checksum"] = serde_json::Value::from(commit_manifest_checksum(
            manifest["format_version"].as_u64().unwrap() as u8,
            manifest["epoch"].as_u64().unwrap(),
            kind,
            manifest["base_epoch"].as_u64(),
            7,
            &files,
            &sidecars,
            manifest["written_at_ms"].as_u64().unwrap(),
        ));
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("generation"),
            "cross-generation checkpoint must refuse with lineage cause: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn flushed_table_manifest_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = unique_dir(tag);
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                31,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
        (dir, manifest_path)
    }

    #[test]
    fn table_manifest_version_mismatch_refuses_open_and_health() {
        let (dir, manifest_path) = flushed_table_manifest_dir("table-version");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["format_version"] = serde_json::Value::from(9u64);
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("version"),
            "unknown table manifest version must refuse: {err}"
        );
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(
            report
                .lineage_issues
                .iter()
                .any(|m| m.contains("rejected") && m.contains("version")),
            "health must locate the version defect: {:?}",
            report.lineage_issues
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_table_manifest_checksum_refuses_open_and_health() {
        let (dir, manifest_path) = flushed_table_manifest_dir("table-checksum");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["checksum"] = serde_json::Value::from(0u64);
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("checksum mismatch"),
            "tampered table manifest must refuse: {err}"
        );
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(
            report
                .lineage_issues
                .iter()
                .any(|m| m.contains("rejected") && m.contains("checksum")),
            "health must locate the checksum defect: {:?}",
            report.lineage_issues
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_table_manifest_refuses_open_and_health() {
        let (dir, manifest_path) = flushed_table_manifest_dir("table-missing");
        std::fs::remove_file(&manifest_path).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("missing table manifest"),
            "missing table manifest must refuse: {err}"
        );
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(
            report
                .lineage_issues
                .iter()
                .any(|m| m.contains("table manifest missing")),
            "health must report the missing file, not a generic defect: {:?}",
            report.lineage_issues
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_manifest_version_mismatch_refuses_open_and_health() {
        let dir = unique_dir("commit-version");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                33,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["format_version"] = serde_json::Value::from(9u64);
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("version"),
            "unknown commit manifest version must refuse: {err}"
        );
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(
            report.manifest_present && !report.manifest_decodable,
            "undecodable commit manifest must stay visible: {report:?}"
        );
        assert!(
            report
                .lineage_issues
                .iter()
                .any(|m| m.contains("undecodable")),
            "health must flag the unprovable lineage: {:?}",
            report.lineage_issues
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staged_redistribution_adopt_checks_lineage() {
        let staging = unique_dir("reshard-stage");
        let _ = std::fs::remove_dir_all(&staging);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        for i in 0..10 {
            table
                .insert(
                    &format!("s_{i}"),
                    &[("name".to_string(), Value::from(format!("s_{i}")))],
                    10,
                )
                .unwrap();
        }
        let receipt = table
            .redistribute_to_staging(&staging, 4, CompressionType::Zstd { level: 0 })
            .expect("staging succeeds");
        assert_eq!(receipt.source_generation, 0);
        assert_eq!(receipt.target_generation, 1);
        assert_eq!((receipt.source_shards, receipt.target_shards), (2, 4));
        assert_eq!(receipt.rows, 10);
        assert_eq!(receipt.mappings, 10);
        let checked = ShardedVertexTable::check_staged_redistribution(&staging, 0)
            .expect("lineage continuity proves");
        assert_eq!(checked, receipt);
        let adopted = ShardedVertexTable::with_layout(
            1,
            "t".to_string(),
            test_schema(),
            super::super::routing::ShardLayout::for_new_table(4),
            1,
        );
        adopted.load(&staging).expect("adopted lineage opens");
        assert_eq!(adopted.approximate_total_count(), 10);
        let rebuilt_ts = graphdb_core::types::MAX_TIMESTAMP - 1;
        assert!(adopted.get_internal_id("s_3", rebuilt_ts).is_some());
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn staged_redistribution_refuses_broken_lineage() {
        let staging = unique_dir("reshard-broken");
        let _ = std::fs::remove_dir_all(&staging);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .redistribute_to_staging(&staging, 4, CompressionType::Zstd { level: 0 })
            .expect("staging succeeds");
        let err = ShardedVertexTable::check_staged_redistribution(&staging, 5)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("lineage continuity"),
            "wrong source generation must refuse adopt: {err}"
        );
        let receipt_path = staging.join(RESHARD_RECEIPT_FILE_NAME);
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        receipt["target_generation"] = serde_json::Value::from(9u64);
        std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let err = ShardedVertexTable::check_staged_redistribution(&staging, 0)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("lineage continuity"),
            "tampered target generation must refuse adopt: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn staged_redistribution_refuses_unhealthy_staging() {
        for (tag, tamper) in [
            ("missing-manifest", "commit manifest missing"),
            ("corrupt-pk", "shard_0"),
        ] {
            let staging = unique_dir(&format!("reshard-sick-{tag}"));
            let _ = std::fs::remove_dir_all(&staging);
            let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
            table
                .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
                .unwrap();
            table
                .redistribute_to_staging(&staging, 4, CompressionType::Zstd { level: 0 })
                .expect("staging succeeds");
            if tag == "missing-manifest" {
                std::fs::remove_file(staging.join(COMMIT_MANIFEST_FILE_NAME)).unwrap();
            } else {
                std::fs::write(staging.join("shard_0").join("id_indexer.bin"), b"corrupt").unwrap();
            }
            let err = ShardedVertexTable::check_staged_redistribution(&staging, 0)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("unhealthy") && err.contains(tamper),
                "damaged staging must refuse adopt with located cause: {err}"
            );
            let _ = std::fs::remove_dir_all(&staging);
        }
    }

    #[test]
    fn health_report_proves_lineage_on_healthy_checkpoint() {
        let dir = unique_dir("health-lineage");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                27,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.is_healthy());
        assert_eq!(report.router_version, Some(1));
        assert_eq!(report.generation, Some(0));
        assert_eq!(report.commit_generation, Some(0));
        assert!(report.lineage_issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_report_flags_generation_mismatch_before_open() {
        let dir = unique_dir("health-mismatch");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                29,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["generation"] = serde_json::Value::from(7u64);
        let kind = match manifest["kind"].as_str().unwrap() {
            "full" => CommitKind::Full,
            "incremental" => CommitKind::Incremental,
            other => panic!("unexpected commit kind {other}"),
        };
        let files: Vec<String> = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let sidecars: Vec<SnapshotSidecarRecord> =
            serde_json::from_value(manifest.get("sidecars").cloned().unwrap_or_default())
                .unwrap_or_default();
        manifest["checksum"] = serde_json::Value::from(commit_manifest_checksum(
            manifest["format_version"].as_u64().unwrap() as u8,
            manifest["epoch"].as_u64().unwrap(),
            kind,
            manifest["base_epoch"].as_u64(),
            7,
            &files,
            &sidecars,
            manifest["written_at_ms"].as_u64().unwrap(),
        ));
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(!report.is_healthy());
        assert!(report.manifest_decodable);
        assert_eq!(report.commit_generation, Some(7));
        assert!(
            report.lineage_issues.iter().any(|m| m.contains("differs")),
            "mismatch must be named before open refuses: {:?}",
            report.lineage_issues
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_signals_track_delta_and_baseline_age() {
        let dir = unique_dir("flush-signals");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        let pending = table.flush_signals();
        assert!(pending.delta_entries > 0);
        assert_eq!(pending.millis_since_baseline, u64::MAX);
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                31,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let anchored = table.flush_signals();
        assert_eq!(anchored.delta_entries, 0);
        assert!(anchored.millis_since_baseline < 60_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn baseline_timestamp_survives_restart_and_drives_age() {
        let dir = unique_dir("baseline-ts");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                41,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(TABLE_MANIFEST_FILE_NAME)).unwrap())
                .unwrap();
        assert!(
            manifest.get("last_full_flush_ms").is_some(),
            "full flush must persist the baseline timestamp"
        );
        let reopened = ShardedVertexTable::with_layout(
            1,
            "t".to_string(),
            test_schema(),
            super::super::routing::ShardLayout::for_new_table(2),
            0,
        );
        reopened.load(&dir).expect("timestamped manifest opens");
        let signals = reopened.flush_signals();
        assert!(
            signals.millis_since_baseline < 60_000,
            "restarted age signal must be continuous, got {}",
            signals.millis_since_baseline
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_without_timestamp_still_opens() {
        let dir = unique_dir("baseline-ts-missing");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                43,
                CommitKind::Full,
                None,
            )
            .unwrap();
        // Strip the timestamp and re-sign with the legacy checksum: an old
        // manifest must still open with a fallback estimate, not refuse.
        let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest
            .as_object_mut()
            .unwrap()
            .remove("last_full_flush_ms");
        manifest["checksum"] =
            serde_json::Value::from(legacy_table_manifest_checksum(TableManifestInput {
                format_version: manifest["format_version"].as_u64().unwrap() as u8,
                label: manifest["label"].as_u64().unwrap() as graphdb_core::types::LabelId,
                label_name: manifest["label_name"].as_str().unwrap(),
                num_shards: manifest["num_shards"].as_u64().unwrap() as usize,
                segment_slots_bits: manifest["segment_slots_bits"].as_u64().unwrap() as u32,
                total_segments: manifest["total_segments"].as_u64().unwrap() as u32,
                router_version: manifest["router_version"].as_u64().unwrap() as u8,
                generation: manifest["generation"].as_u64().unwrap(),
                last_full_flush_ms: None,
            }));
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let reopened = ShardedVertexTable::with_layout(
            1,
            "t".to_string(),
            test_schema(),
            super::super::routing::ShardLayout::for_new_table(2),
            0,
        );
        reopened
            .load(&dir)
            .expect("old manifest opens with fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_plan_advises_baseline_before_first_full_flush() {
        use crate::vertex::vertex_table::flush_trigger::{FlushKind, FlushReason};
        let dir = unique_dir("flush-plan");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
            .unwrap();
        // No baseline has ever been anchored in this process: the unknown
        // age reads as infinitely old, so the coordinator must escalate.
        let plan = table.flush_plan();
        assert_eq!(plan.kind, FlushKind::Full);
        assert_eq!(plan.reason, FlushReason::BaselineAge);
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                37,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let settled = table.flush_plan();
        assert_eq!(settled.kind, FlushKind::Skip);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_store_inspection_aggregates_labels_and_chain() {
        let root = unique_dir("store-health");
        let _ = std::fs::remove_dir_all(&root);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                root.join("label_1"),
                CompressionType::Zstd { level: 0 },
                31,
                CommitKind::Full,
                None,
            )
            .unwrap();
        table
            .flush_incremental_with_epoch(
                root.join("label_2"),
                CompressionType::Zstd { level: 0 },
                32,
                Some(31),
            )
            .unwrap();
        let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
        assert_eq!(health.tables.len(), 2);
        assert!(health.is_healthy());
        assert!(health.chain_ok);
        assert!(health.issues.is_empty());
        std::fs::write(root.join("label_2").join("half.tmp"), b"x").unwrap();
        let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
        assert!(health.is_healthy());
        let manifest = ShardedVertexTable::read_commit_manifest(root.join("label_1"))
            .unwrap()
            .expect("manifest");
        let victim = manifest.files.first().expect("listed file").clone();
        std::fs::remove_file(root.join("label_1").join(&victim)).unwrap();
        let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
        assert!(!health.is_healthy());
        assert!(health.issues.iter().any(|m| m.contains("missing")));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_enveloped_delta(path: &std::path::Path, raw: &[u8]) {
        use crate::persistence::{section, write_header_to};
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_ID_INDEXER_DELTA).unwrap();
        payload.extend_from_slice(raw);
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let mut writer = crate::compression::PageWriter::new(page_size, 3);
        let mut pages_buf = Vec::new();
        writer.write_all(&mut pages_buf, &payload).unwrap();
        let mut final_buf = Vec::new();
        crate::compression::ColumnFileHeader {
            page_size,
            page_count: writer.page_count(),
            total_rows: 1,
        }
        .serialize(&mut final_buf)
        .unwrap();
        final_buf.extend_from_slice(&pages_buf);
        crate::compression::write_shadow_file(path, &final_buf).unwrap();
    }

    #[test]
    fn pk_baseline_corrupt_refuses_open_and_health() {
        let dir = unique_dir("pk-base-corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                41,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.is_healthy());
        assert!(report.pk_index_ok);
        assert!(report.pk_issues.is_empty());

        std::fs::write(dir.join("shard_0").join("id_indexer.bin"), b"corrupt").unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("41") && err.contains("shard"),
            "baseline corruption must refuse with epoch and shard: {err}"
        );
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(!report.is_healthy());
        assert!(!report.pk_index_ok);
        assert!(report.pk_issues.iter().any(|m| m.contains("pk baseline")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pk_diverging_delta_refuses_apply_and_health() {
        use crate::vertex::id_indexer::{IdKey, IdManager};

        let base = unique_dir("pk-div-base");
        let incr = unique_dir("pk-div-incr");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &base,
                CompressionType::Zstd { level: 0 },
                42,
                CommitKind::Full,
                None,
            )
            .unwrap();
        table
            .insert("v2", &[("name".to_string(), Value::from("v2"))], ts)
            .unwrap();
        table
            .flush_incremental_with_epoch(&incr, CompressionType::Zstd { level: 0 }, 43, Some(42))
            .unwrap();

        let mut mgr = IdManager::new();
        mgr.insert(IdKey::Text("v1".to_string())).unwrap();
        let mut raw = mgr.serialize_delta();
        raw[5..9].copy_from_slice(&7u32.to_le_bytes());
        write_enveloped_delta(&incr.join("shard_0").join("id_indexer.delta"), &raw);

        let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        strict.load(&base).unwrap();
        let err = strict.apply_delta_pages(&incr).unwrap_err().to_string();
        assert!(
            err.contains("diverges"),
            "divergent delta must refuse on divergence: {err}"
        );
        // Per-directory health only checks decodability here (the anchor
        // lives in the base directory); the divergence refuses at apply.
        let report = ShardedVertexTable::inspect_commit_health(&incr).unwrap();
        assert!(report.pk_index_ok);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
    }

    #[test]
    fn pk_lingering_delta_flagged_by_health() {
        use crate::vertex::id_indexer::{IdKey, IdManager};

        let dir = unique_dir("pk-linger");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                44,
                CommitKind::Full,
                None,
            )
            .unwrap();

        let mut mgr = IdManager::new();
        mgr.insert(IdKey::Text("v1".to_string())).unwrap();
        let mut raw = mgr.serialize_delta();
        raw[5..9].copy_from_slice(&7u32.to_le_bytes());
        write_enveloped_delta(&dir.join("shard_0").join("id_indexer.delta"), &raw);

        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(!report.is_healthy());
        assert!(!report.pk_index_ok);
        assert!(report.pk_issues.iter().any(|m| m.contains("diverges")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_sidecars_stay_outside_manifest_and_load() {
        let dir = unique_dir("snap-manifest");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                45,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest = ShardedVertexTable::read_commit_manifest(&dir)
            .unwrap()
            .expect("commit manifest present");
        assert!(manifest.files.iter().all(|f| !f.ends_with(".snapshot")));

        std::fs::write(dir.join("shard_0").join("name.snapshot"), b"junk").unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v1", ts).is_some());
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        // Unpinned junk is discardable: healthy for strict open, visible as
        // a sidecar issue for observability.
        assert!(report.is_healthy());
        assert!(!report.sidecar_issues.is_empty());
        assert_eq!(report.sidecar_discards(), report.sidecar_issues.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinned_sidecar_corruption_still_opens_with_discard() {
        let dir = unique_dir("snap-pinned");
        let _ = std::fs::remove_dir_all(&dir);
        let schema = crate::vertex::VertexSchema {
            label_id: 1,
            label_name: "person".to_string(),
            properties: vec![
                crate::types::StoragePropertyDef::new("name".to_string(), DataType::String),
                crate::types::StoragePropertyDef::new("group".to_string(), DataType::String),
            ],
            primary_key_index: 0,
            schema_version: 1,
        };
        let table = ShardedVertexTable::with_config(1, "t".to_string(), schema, 1);
        let ts: Timestamp = 10;
        for i in 0..5000 {
            let id = format!("v{i:05}");
            table
                .insert(
                    &id,
                    &[
                        ("name".to_string(), Value::from(id.clone())),
                        ("group".to_string(), Value::from("same")),
                    ],
                    ts,
                )
                .unwrap();
        }
        // First flush encodes in-memory chunks; eviction needs encoded
        // chunks, so force-encode the low-cardinality column and evict it
        // directly, then flush again to pin.
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                46,
                CommitKind::Full,
                None,
            )
            .unwrap();
        table.force_encode_evict_for_test("group");
        let _ = table.evict_cold_chunks(u64::MAX);
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                47,
                CommitKind::Full,
                None,
            )
            .unwrap();
        let manifest = ShardedVertexTable::read_commit_manifest(&dir)
            .unwrap()
            .expect("commit manifest present");
        assert!(
            !manifest.sidecars.is_empty(),
            "evicted flush must pin sidecars"
        );
        // Corrupt the first pinned sidecar: strict open must still succeed
        // with resident fallback, and health must flag the discard.
        let victim = dir.join(&manifest.sidecars[0].file);
        std::fs::write(&victim, b"tampered").unwrap();
        let reload_schema = crate::vertex::VertexSchema {
            label_id: 1,
            label_name: "person".to_string(),
            properties: vec![
                crate::types::StoragePropertyDef::new("name".to_string(), DataType::String),
                crate::types::StoragePropertyDef::new("group".to_string(), DataType::String),
            ],
            primary_key_index: 0,
            schema_version: 1,
        };
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), reload_schema, 1);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v00042", ts).is_some());
        let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
        assert!(report.is_healthy());
        assert!(!report.sidecar_issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_mode_isolates_single_shard_damage() {
        let dir = unique_dir("repair-isolate");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
            .unwrap();
        table
            .flush_with_epoch(
                &dir,
                CompressionType::Zstd { level: 0 },
                47,
                CommitKind::Full,
                None,
            )
            .unwrap();
        // Corrupt one shard's authoritative pages: strict open refuses with
        // a machine-readable isolatable class, repair opens the healthy
        // shard and reports the damaged one.
        std::fs::write(dir.join("shard_0").join("columns.bin"), b"corrupt").unwrap();
        let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = strict.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("class=isolatable") && err.contains("shard"),
            "strict shard failure must carry machine-readable class and shard: {err}"
        );
        let probe = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let report = probe.load_for_repair(&dir).unwrap();
        assert_eq!(report.healthy_shards, vec![1]);
        assert_eq!(report.damaged_shards.len(), 1);
        assert_eq!(report.damaged_shards[0].shard, 0);
        assert_eq!(
            report.damaged_shards[0].class,
            super::CorruptionClass::Isolatable
        );
        assert!(!report.is_complete());
        // Fatal defects still refuse repair mode with class=fatal.
        std::fs::remove_file(dir.join(COMMIT_MANIFEST_FILE_NAME)).unwrap();
        let fatal_probe = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = fatal_probe.load_for_repair(&dir).unwrap_err().to_string();
        assert!(
            err.contains("class=fatal"),
            "missing manifest must refuse repair with fatal class: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
