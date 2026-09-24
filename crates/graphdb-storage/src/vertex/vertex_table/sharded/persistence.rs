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
    label: graphdb_core::types::LabelId,
    label_name: String,
    num_shards: usize,
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
    pub(crate) epoch: u64,
    pub(crate) kind: CommitKind,
    pub(crate) base_epoch: Option<u64>,
    pub(crate) files: Vec<String>,
    pub(crate) written_at_ms: u64,
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
        let manifest = TableManifest {
            label: self.label,
            label_name: self.label_name.clone(),
            num_shards: self.num_shards,
        };
        let payload = serde_json::to_vec(&manifest)
            .map_err(|e| graphdb_core::StorageError::serialize_error(e.to_string()))?;
        crate::compression::write_shadow_file(
            path.as_ref().join(TABLE_MANIFEST_FILE_NAME),
            &payload,
        )
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
        Ok(Some(manifest))
    }

    fn write_commit_manifest<P: AsRef<Path>>(
        path: P,
        epoch: u64,
        kind: CommitKind,
        base_epoch: Option<u64>,
    ) -> StorageResult<()> {
        let files = collect_committed_files(path.as_ref())?;
        let manifest = CommitManifest {
            epoch,
            kind,
            base_epoch,
            files,
            written_at_ms: now_ms(),
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

    fn check_table_manifest<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let manifest = Self::read_table_manifest(&path)?;
        let Some(manifest) = manifest else {
            return Ok(());
        };
        if manifest.num_shards != self.num_shards {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "vertex table '{}' persisted with num_shards={} but opened with num_shards={} \
                 (manifest {}): global internal IDs embed the shard count and would \
                 mis-decode; reopen with vertex_table_shards={} or migrate the data",
                self.label_name,
                manifest.num_shards,
                self.num_shards,
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
        // Refuse to mis-decode: persisted global IDs embed the shard count.
        self.check_table_manifest(&path)?;
        match Self::read_commit_manifest(&path)? {
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
            None => {
                // Legacy path without a commit manifest: keep loadable.
                for (i, shard) in self.shards.iter().enumerate() {
                    let shard_dir = path.join(format!("shard_{}", i));
                    if shard_dir.exists() {
                        let mut table = shard.write();
                        table.load(&shard_dir)?;
                    } else {
                        let _ = i;
                    }
                }
                Ok(())
            }
        }
    }

    pub fn apply_delta_pages<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        match Self::read_commit_manifest(&path)? {
            Some(manifest) => self.apply_delta_pages_strict(path, &manifest),
            None => self.apply_delta_pages_legacy(path),
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
                table.apply_delta_pages_strict(&shard_dir).map_err(|e| {
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

    fn apply_delta_pages_legacy(&self, path: &Path) -> StorageResult<()> {
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            let has_delta = shard_dir.join("columns_pages").exists()
                || shard_dir.join("timestamps.bin").exists()
                || shard_dir.join("id_indexer.bin").exists()
                || shard_dir.join("id_indexer.delta").exists();
            if has_delta && shard_dir.exists() {
                let mut table = shard.write();
                // Apply column delta pages if any (corrupted pages are skipped internally)
                if shard_dir.join("columns_pages").exists() {
                    if let Err(e) = table.apply_delta_pages(&shard_dir) {
                        log::warn!(
                            "Failed to apply delta pages for shard {}: {}, falling back to base",
                            i,
                            e
                        );
                    }
                }
                // For incremental, timestamps and id_indexer are flushed fully; reload them
                let ts_path = shard_dir.join("timestamps.bin");
                if ts_path.exists() {
                    if let Err(e) = table.load_timestamps(&ts_path) {
                        log::warn!(
                            "Failed to load timestamps for shard {} from {}: {}",
                            i,
                            ts_path.display(),
                            e
                        );
                    }
                }
                let id_path = shard_dir.join("id_indexer.bin");
                if id_path.exists() {
                    if let Err(e) = table.load_id_indexer(&id_path) {
                        log::warn!(
                            "Failed to load id_indexer for shard {} from {}: {}",
                            i,
                            id_path.display(),
                            e
                        );
                    }
                } else {
                    // A corrupt delta alone never affects the baseline: it is
                    // discarded and rebuilt by the next flush.
                    let delta_path = shard_dir.join("id_indexer.delta");
                    if delta_path.exists() {
                        if let Err(e) = table.load_id_indexer_delta(&delta_path) {
                            log::warn!(
                                "Discarding corrupt id_indexer delta for shard {} from {}: {}",
                                i,
                                delta_path.display(),
                                e
                            );
                        }
                    }
                }
            }
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
    fn missing_commit_manifest_stays_legacy_loadable() {
        let dir = unique_dir("legacy");
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts: Timestamp = 10;
        table
            .insert(
                "v_legacy",
                &[("name".to_string(), Value::from("v_legacy"))],
                ts,
            )
            .unwrap();
        table
            .flush(&dir, CompressionType::Zstd { level: 0 })
            .unwrap();
        std::fs::remove_file(dir.join(COMMIT_MANIFEST_FILE_NAME)).unwrap();
        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v_legacy", ts).is_some());
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
}
