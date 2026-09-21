use std::path::Path;

use super::ShardedVertexTable;
use crate::compression::CompressionType;
use graphdb_core::StorageResult;

impl ShardedVertexTable {
    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        use rayon::prelude::*;
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        self.shards
            .par_iter()
            .enumerate()
            .try_for_each(|(i, shard)| {
                let shard_dir = path.join(format!("shard_{}", i));
                shard.write().flush(&shard_dir, compression)
            })?;
        Ok(())
    }

    pub fn flush_incremental<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        use rayon::prelude::*;
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
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
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            if shard_dir.exists() {
                let mut table = shard.write();
                table.load(&shard_dir)?;
            }
        }
        Ok(())
    }

    pub fn apply_delta_pages<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            let has_delta = shard_dir.join("columns_pages").exists()
                || shard_dir.join("timestamps.bin").exists()
                || shard_dir.join("id_indexer.bin").exists();
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
                }
            }
        }
        Ok(())
    }
}
