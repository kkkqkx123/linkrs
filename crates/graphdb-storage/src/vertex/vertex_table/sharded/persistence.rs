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
    checksum: u32,
}

/// Persistent layout version of both manifests. Version 2 pins the full
/// shard layout (shard count plus segment slot width) that global internal
/// IDs are encoded with. Version 1 manifests carry only the shard count and
/// are rejected with a rebuild directive; there is no automatic migration
/// and unknown versions are rejected the same way.
const MANIFEST_FORMAT_VERSION: u8 = 2;

fn table_manifest_checksum(
    format_version: u8,
    label: graphdb_core::types::LabelId,
    label_name: &str,
    num_shards: usize,
    segment_slots_bits: u32,
) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[format_version]);
    hasher.update(&label.to_le_bytes());
    hasher.update(label_name.as_bytes());
    hasher.update(&(num_shards as u64).to_le_bytes());
    hasher.update(&segment_slots_bits.to_le_bytes());
    hasher.finalize()
}

fn commit_manifest_checksum(
    format_version: u8,
    epoch: u64,
    kind: CommitKind,
    base_epoch: Option<u64>,
    files: &[String],
    written_at_ms: u64,
) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[format_version]);
    hasher.update(&epoch.to_le_bytes());
    hasher.update(kind.as_str().as_bytes());
    hasher.update(&base_epoch.unwrap_or(u64::MAX).to_le_bytes());
    for file in files {
        hasher.update(file.as_bytes());
        hasher.update(&[0]);
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
    let expected = table_manifest_checksum(
        manifest.format_version,
        manifest.label,
        &manifest.label_name,
        manifest.num_shards,
        manifest.segment_slots_bits,
    );
    if expected != manifest.checksum {
        return Err(graphdb_core::StorageError::deserialize_error(format!(
            "table manifest checksum mismatch at {}: expected {:#010x}, got {:#010x}",
            path.display(),
            expected,
            manifest.checksum,
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
        &manifest.files,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CommitManifest {
    pub(crate) format_version: u8,
    pub(crate) epoch: u64,
    pub(crate) kind: CommitKind,
    pub(crate) base_epoch: Option<u64>,
    pub(crate) files: Vec<String>,
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
}

impl CommitHealthReport {
    /// Whether the directory is safe to open strictly: a decodable manifest
    /// with no missing files and a verifiable primary-key index.
    pub fn is_healthy(&self) -> bool {
        self.manifest_present
            && self.manifest_decodable
            && self.missing_files.is_empty()
            && self.pk_index_ok
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

impl GlobalCommitHealth {
    /// Whether every table is healthy and the epoch chain is continuous.
    pub fn is_healthy(&self) -> bool {
        self.chain_ok && self.tables.iter().all(|(_, r)| r.is_healthy())
    }
}

fn now_ms() -> u64 {
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

impl ShardedVertexTable {
    fn write_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let format_version = MANIFEST_FORMAT_VERSION;
        let checksum = table_manifest_checksum(
            format_version,
            self.label,
            &self.label_name,
            self.layout.num_shards,
            self.layout.segment_slots_bits,
        );
        let manifest = TableManifest {
            format_version,
            label: self.label,
            label_name: self.label_name.clone(),
            num_shards: self.layout.num_shards,
            segment_slots_bits: self.layout.segment_slots_bits,
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
    /// Opening a table adopts this layout: the running configuration's
    /// shard count only applies to newly created tables. A missing manifest
    /// yields `None` (the caller keeps its configured layout and the strict
    /// load below refuses the open); an unknown version or checksum failure
    /// errors with a rebuild directive instead of auto-migrating.
    pub(crate) fn manifest_layout<P: AsRef<Path>>(
        path: P,
    ) -> StorageResult<Option<super::routing::ShardLayout>> {
        let Some(manifest) = Self::read_table_manifest(&path)? else {
            return Ok(None);
        };
        Ok(Some(super::routing::ShardLayout {
            num_shards: manifest.num_shards,
            segment_slots_bits: manifest.segment_slots_bits,
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
        path: P,
        epoch: u64,
        kind: CommitKind,
        base_epoch: Option<u64>,
    ) -> StorageResult<()> {
        let files = collect_committed_files(path.as_ref())?;
        let format_version = MANIFEST_FORMAT_VERSION;
        let written_at_ms = now_ms();
        let checksum = commit_manifest_checksum(
            format_version,
            epoch,
            kind,
            base_epoch,
            &files,
            written_at_ms,
        );
        let manifest = CommitManifest {
            format_version,
            epoch,
            kind,
            base_epoch,
            files,
            written_at_ms,
            checksum,
        };
        let payload = serde_json::to_vec(&manifest)
            .map_err(|e| graphdb_core::StorageError::serialize_error(e.to_string()))?;
        crate::compression::write_shadow_file(commit_manifest_path(path.as_ref()), &payload)
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
        let mut listed_files = Vec::new();
        let mut missing_files = Vec::new();
        if manifest_present {
            if let Ok(payload) = std::fs::read(&manifest_path) {
                if let Ok(manifest) = serde_json::from_slice::<CommitManifest>(&payload) {
                    if verify_commit_manifest_content(&manifest, &manifest_path).is_ok() {
                        manifest_decodable = true;
                        epoch = Some(manifest.epoch);
                        kind = Some(manifest.kind.as_str().to_string());
                        base_epoch = manifest.base_epoch;
                        listed_files = manifest.files.clone();
                        for rel in &manifest.files {
                            if !dir.join(rel).exists() {
                                missing_files.push(rel.clone());
                            }
                        }
                    }
                }
            }
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
        Ok(CommitHealthReport {
            manifest_present,
            manifest_decodable,
            epoch,
            kind,
            base_epoch,
            listed_files,
            missing_files,
            orphan_tmp_files,
            pk_index_ok,
            pk_issues,
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
        {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "vertex table '{}' persisted with layout (num_shards={}, segment_slots_bits={}) \
                 but opened with layout (num_shards={}, segment_slots_bits={}) \
                 (manifest {}): global internal IDs embed the shard layout and would \
                 mis-decode; reopen with vertex_table_shards={} or migrate the data with \
                 the offline redistribution tool",
                self.label_name,
                manifest.num_shards,
                manifest.segment_slots_bits,
                self.layout.num_shards,
                self.layout.segment_slots_bits,
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
        Ok(())
    }

    fn verify_commit_manifest(&self, path: &Path, manifest: &CommitManifest) -> StorageResult<()> {
        for rel in &manifest.files {
            let full = path.join(rel);
            if !full.exists() {
                return Err(graphdb_core::StorageError::deserialize_error(format!(
                    "checkpoint epoch {} kind={} incomplete: manifest-listed file missing: {}",
                    manifest.epoch,
                    manifest.kind.as_str(),
                    full.display(),
                )));
            }
        }
        Ok(())
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
        self.write_table_manifest(path)?;
        Self::write_commit_manifest(path, epoch, kind, base_epoch)?;
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
        Self::write_commit_manifest(path, epoch, CommitKind::Incremental, base_epoch)?;
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
        match Self::read_commit_manifest(path)? {
            Some(manifest) => {
                self.verify_commit_manifest(path, &manifest)?;
                for (i, shard) in self.shards.iter().enumerate() {
                    let shard_dir = path.join(format!("shard_{}", i));
                    if !shard_dir.exists() {
                        return Err(graphdb_core::StorageError::deserialize_error(format!(
                            "checkpoint epoch {} incomplete: shard directory missing: {}",
                            manifest.epoch,
                            shard_dir.display(),
                        )));
                    }
                    let mut table = shard.write();
                    table.load(&shard_dir).map_err(|e| {
                        graphdb_core::StorageError::deserialize_error(format!(
                            "checkpoint epoch {} shard {} corrupt at {}: {}",
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
                "vertex table '{}' missing commit manifest at {}: refusing open without checkpoint pin",
                self.label_name,
                path.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))),
        }
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
        assert!(report.is_healthy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
