use std::path::Path;

use crate::compression::CompressionType;
use graphdb_core::StorageResult;

use super::super::core::VertexTable;

impl VertexTable {
    /// Incremental flush: only serialize dirty pages.
    pub fn flush_incremental<P: AsRef<Path>>(
        &mut self,
        path: P,
        dirty_pages: &[crate::persistence::dirty_page::PageId],
        compression: crate::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;

        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::compression::cleanup_shadow_files(path)?;

        let CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;

        // Always flush meta (small overhead, needed for base checkpoint reference).
        let meta_path = path.join("meta.bin");
        let meta_payload = self.build_meta_payload()?;
        Self::write_pages_to_file(&meta_path, &meta_payload, page_size, level, 1)?;

        // Determine dirty pages to flush. If caller supplied an explicit list,
        // respect it; otherwise collect from column dirty tracking.
        let effective_dirty: Vec<crate::persistence::dirty_page::PageId> = if dirty_pages.is_empty()
        {
            self.dirty_pages()
        } else {
            dirty_pages.to_vec()
        };

        // Flush only dirty column pages into a delta directory.
        let flushed: Vec<(String, usize)> = if !effective_dirty.is_empty() {
            self.flush_dirty_column_pages(path, &effective_dirty)?
        } else {
            // No dirty pages: still ensure columns.bin exists for incremental base?
            // We create an empty delta marker so checkpoint is not considered corrupt.
            let delta_dir = path.join("columns_pages");
            fs::create_dir_all(&delta_dir)?;
            Vec::new()
        };

        let timestamps_path = path.join("timestamps.bin");
        self.flush_timestamps(&timestamps_path)?;

        // Primary-key baseline plus delta: an invalidated or over-threshold
        // baseline is rewritten in full as the new anchor (superseding any
        // delta); otherwise only the since-baseline delta is persisted
        // alongside the column delta pages under the same checkpoint commit.
        if self.id_indexer.should_anchor_baseline() {
            let id_indexer_path = path.join("id_indexer.bin");
            self.flush_id_indexer_baseline(path, &id_indexer_path)?;
        } else if self.id_indexer.delta_len() > 0 {
            let delta_path = path.join("id_indexer.delta");
            self.flush_id_indexer_delta(&delta_path)?;
        } else {
            let delta_path = path.join("id_indexer.delta");
            if delta_path.exists() {
                std::fs::remove_file(&delta_path)?;
            }
        }

        // Clear the dirty mark only for pages actually written this round, so
        // pages skipped (e.g. filtered out of an externally supplied list) stay
        // tracked for the next flush instead of being silently lost.
        self.columns.clear_pages(&flushed);

        Ok(())
    }

    fn flush_dirty_column_pages(
        &self,
        path: &Path,
        dirty_pages: &[crate::persistence::dirty_page::PageId],
    ) -> StorageResult<Vec<(String, usize)>> {
        use rayon::prelude::*;
        use std::collections::HashSet;
        let delta_dir = path.join("columns_pages");
        std::fs::create_dir_all(&delta_dir)?;

        // Build set of dirty page ids for fast lookup (row-page granularity).
        let dirty_set: HashSet<u64> = dirty_pages.iter().map(|p| p.page_id).collect();

        // Collect all (col_name, page_id, bytes) to flush in parallel
        let mut tasks: Vec<(String, usize, Vec<u8>)> = Vec::new();
        for col_name in self.columns.column_names() {
            let Some(col) = self.columns.get_column(&col_name) else {
                continue;
            };
            let col_dirty = col.dirty_pages();
            for page_id in col_dirty {
                if !dirty_set.contains(&(page_id as u64)) {
                    continue;
                }
                // Sparse schemas leave never-written columns shorter than
                // their dirty marks (e.g. a delete dirties every column):
                // out-of-range pages carry no rows and are skipped.
                if page_id * crate::persistence::dirty_page::ROWS_PER_PAGE >= col.len() {
                    continue;
                }
                let page_bytes = col.serialize_page(page_id)?;
                tasks.push((col.name.clone(), page_id, page_bytes));
            }
        }
        // Handle externally supplied dirty_pages when column tracking is empty
        if tasks.is_empty() && !dirty_pages.is_empty() {
            for page_id in dirty_pages {
                if page_id.component != crate::persistence::dirty_page::ComponentType::VertexColumns
                {
                    continue;
                }
                let pid = page_id.page_id as usize;
                for col_name in self.columns.column_names() {
                    let Some(col) = self.columns.get_column(&col_name) else {
                        continue;
                    };
                    if pid * crate::persistence::dirty_page::ROWS_PER_PAGE >= col.len() {
                        continue;
                    }
                    if let Ok(bytes) = col.serialize_page(pid) {
                        tasks.push((col.name.clone(), pid, bytes));
                    }
                }
            }
        }

        // Deduplicate tasks by (col_name, page_id)
        {
            use std::collections::HashSet as Set2;
            let mut seen = Set2::new();
            tasks.retain(|(name, pid, _)| seen.insert((name.clone(), *pid)));
        }

        // Parallel write using rayon
        let delta_dir_clone = delta_dir.clone();
        tasks
            .par_iter()
            .try_for_each(|(col_name, page_id, bytes)| {
                let file_name = format!("{}_{}.page", col_name, page_id);
                let page_path = delta_dir_clone.join(file_name);
                crate::compression::write_shadow_file(&page_path, bytes)
            })?;

        Ok(tasks
            .into_iter()
            .map(|(name, pid, _)| (name, pid))
            .collect())
    }

    pub(super) fn write_pages_to_file(
        path: &Path,
        payload: &[u8],
        page_size: usize,
        level: i32,
        total_rows: u32,
    ) -> StorageResult<()> {
        let mut pages_buf = Vec::new();
        let mut writer = crate::compression::PageWriter::new(page_size, level);
        writer.write_all(&mut pages_buf, payload)?;

        let mut final_buf = Vec::new();
        let header = crate::compression::ColumnFileHeader {
            page_size,
            page_count: writer.page_count(),
            total_rows,
        };
        header.serialize(&mut final_buf)?;
        final_buf.extend_from_slice(&pages_buf);

        crate::compression::write_shadow_file(path, &final_buf)
    }
}
