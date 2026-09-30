//! Checkpoint writes: full plus incremental flush with orphan sweep.

use std::path::Path;

use super::super::ShardedVertexTable;
use super::commit_manifest::CommitKind;
use super::common::now_ms;
use crate::compression::CompressionType;
use graphdb_core::StorageResult;

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
    /// Delete temp/staging leftovers without failing. Orphan files and
    /// manifest-external files are always tolerated; only manifest-listed
    /// content is strict.
    pub fn cleanup_orphans<P: AsRef<Path>>(path: P) {
        cleanup_orphans_tolerant(path.as_ref());
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
        let plan = super::super::super::flush_trigger::decide(self.flush_signals());
        log::debug!(
            "vertex table '{}' incremental flush trigger: kind={:?} reason={} merge_pages={}",
            self.label_name,
            plan.kind,
            plan.reason.as_str(),
            plan.merge_pages,
        );
        let anchor = self.decide_pk_anchor();
        self.shards
            .par_iter()
            .enumerate()
            .try_for_each(|(i, shard)| {
                let shard_dir = path.join(format!("shard_{}", i));
                let mut table = shard.write();
                let dirty: Vec<crate::persistence::dirty_page::PageId> = table.dirty_pages();
                // Clean shards carry no column, timestamp or pk-delta change
                // (every timestamp mutation marks its row dirty), so they
                // write nothing: the replay falls back to the baseline.
                if dirty.is_empty() && table.id_indexer.delta_len() == 0 && !anchor {
                    let _ = std::fs::create_dir_all(&shard_dir);
                    return Ok(());
                }
                if dirty.is_empty() && table.total_count() == 0 {
                    let _ = std::fs::create_dir_all(&shard_dir);
                    return Ok(());
                }
                if dirty.is_empty() {
                    table.flush_incremental_with_anchor(&shard_dir, &[], compression, anchor)
                } else {
                    table.flush_incremental_with_anchor(&shard_dir, &dirty, compression, anchor)
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
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for shard in &self.shards {
            for id in shard.read().dirty_pages() {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        out
    }
}
