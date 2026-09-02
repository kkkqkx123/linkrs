//! Vertex Table Persistence Layer
//!
//! Handles serialization, deserialization, and file I/O for vertex tables.
//!
//! # Encoding Handling
//! - Encodings are serialized as structured metadata during flush
//! - Encodings are reconstructed directly via `deserialize_meta()` on load

use std::io::Read;
use std::path::Path;

use crate::compression::CompressionType;
use crate::encoding::EncodingType;
use crate::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use graphdb_core::{StorageError, StorageResult};

use super::core::VertexTable;

fn take_bytes(cursor: &mut &[u8], len: u32, field: &str) -> StorageResult<Vec<u8>> {
    let len = len as usize;
    if len > cursor.len() {
        return Err(StorageError::deserialize_error(format!(
            "{} length {} exceeds remaining input {}",
            field,
            len,
            cursor.len()
        )));
    }
    let (value, remaining) = cursor.split_at(len);
    *cursor = remaining;
    Ok(value.to_vec())
}

impl VertexTable {
    pub fn flush<P: AsRef<Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;

        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::compression::cleanup_shadow_files(path)?;

        let CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;

        let meta_path = path.join("meta.bin");
        let meta_payload = self.build_meta_payload()?;
        Self::write_pages_to_file(&meta_path, &meta_payload, page_size, level, 1)?;

        let id_indexer_path = path.join("id_indexer.bin");
        self.flush_id_indexer(&id_indexer_path)?;

        let columns_path = path.join("columns.bin");
        // Encoding is a flush-time concern. Work on a snapshot so active
        // writes keep using the unmodified in-memory representation.
        let mut columns = self.columns.clone();
        // Use the persistent encoding selector so compression feedback
        // accumulates across flushes, enabling the re-encoding detector.
        let selections = columns
            .columns()
            .iter()
            .map(|col| {
                let values = (0..col.len())
                    .map(|row_idx| col.get(row_idx))
                    .collect::<Vec<_>>();
                (
                    col.name.clone(),
                    self.encoding_selector
                        .select_for_column(&col.data_type, &values),
                )
            })
            .collect::<Vec<_>>();
        for (name, encoding_type) in &selections {
            if *encoding_type != EncodingType::None {
                columns.apply_encoding_to_column(
                    name,
                    *encoding_type,
                    self.encoding_selector.thresholds().fsst_max_symbols,
                )?;
            }
        }
        for (name, encoding_type) in &selections {
            if *encoding_type != EncodingType::None {
                if let Some(col) = columns.get_column(name) {
                    if let Ok(stats) = col.compute_stats() {
                        log::debug!(
                            "flush column={} encoding={:?} ratio={:.2}% savings={:.2}% raw={} compressed={}",
                            name,
                            encoding_type,
                            stats.compression_ratio() * 100.0,
                            stats.space_savings() * 100.0,
                            stats.raw_size,
                            stats.compressed_size,
                        );
                        self.encoding_selector
                            .record_compression_result(*encoding_type, stats.compression_ratio());
                        if self.encoding_selector.should_reencode(*encoding_type) {
                            log::info!(
                                "column={} encoding={:?} avg_ratio={:.2} exceeds threshold, \
                                 consider re-encoding",
                                name,
                                encoding_type,
                                self.encoding_selector.thresholds().reencode_threshold,
                            );
                        }
                    }
                }
            }
        }
        self.flush_columns(&columns_path, &columns)?;

        // Apply encoding to in-memory columns so data stays compressed after flush.
        // This moves compression from "flush-time only" to "post-flush in-memory",
        // reducing memory footprint for the lifetime of the column store.
        for (name, encoding_type) in &selections {
            if *encoding_type != EncodingType::None {
                if let Err(e) = self.columns.apply_encoding_to_column(
                    name,
                    *encoding_type,
                    self.encoding_selector.thresholds().fsst_max_symbols,
                ) {
                    log::warn!(
                        "failed to apply encoding to in-memory column {}: {}",
                        name,
                        e
                    );
                }
            }
        }

        let timestamps_path = path.join("timestamps.bin");
        self.flush_timestamps(&timestamps_path)?;
        // Successful full flush clears dirty tracking (data now persisted).
        self.clear_dirty();

        Ok(())
    }

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
        if !effective_dirty.is_empty() {
            self.flush_dirty_column_pages(path, &effective_dirty)?;
        } else {
            // No dirty pages: still ensure columns.bin exists for incremental base?
            // We create an empty delta marker so checkpoint is not considered corrupt.
            let delta_dir = path.join("columns_pages");
            fs::create_dir_all(&delta_dir)?;
        }

        let timestamps_path = path.join("timestamps.bin");
        self.flush_timestamps(&timestamps_path)?;

        let id_indexer_path = path.join("id_indexer.bin");
        self.flush_id_indexer(&id_indexer_path)?;

        // Incremental flush also clears dirty marks for flushed pages.
        // We clear all for simplicity; per-page clear would require mapping.
        self.clear_dirty();

        Ok(())
    }

    fn flush_dirty_column_pages(
        &self,
        path: &Path,
        dirty_pages: &[crate::persistence::dirty_page::PageId],
    ) -> StorageResult<()> {
        use rayon::prelude::*;
        use std::collections::HashSet;
        let delta_dir = path.join("columns_pages");
        std::fs::create_dir_all(&delta_dir)?;

        // Build set of dirty page ids for fast lookup (row-page granularity).
        let dirty_set: HashSet<u64> = dirty_pages.iter().map(|p| p.page_id).collect();

        // Collect all (col_name, page_id, bytes) to flush in parallel
        let mut tasks: Vec<(String, usize, Vec<u8>)> = Vec::new();
        for col in self.columns.columns() {
            let col_dirty = col.dirty_pages();
            for page_id in col_dirty {
                if !dirty_set.contains(&(page_id as u64)) {
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
                for col in self.columns.columns() {
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

        Ok(())
    }

    fn write_pages_to_file(
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

    fn build_meta_payload(&self) -> StorageResult<Vec<u8>> {
        let mut buf = Vec::new();
        write_header_to(&mut buf, section::VERTEX_META)
            .map_err(|e| StorageError::io_error(format!("Failed to write meta header: {}", e)))?;

        let label_bytes = self.label.to_le_bytes();
        let label_name_bytes = self.label_name.as_bytes();
        let label_name_len = label_name_bytes.len() as u32;

        buf.extend_from_slice(&label_bytes);
        buf.extend_from_slice(&label_name_len.to_le_bytes());
        buf.extend_from_slice(label_name_bytes);

        let schema_json = serde_json::to_string(&self.schema)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let schema_bytes = schema_json.as_bytes();
        buf.extend_from_slice(&(schema_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(schema_bytes);

        Ok(buf)
    }

    fn flush_id_indexer(&self, path: &Path) -> StorageResult<()> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_ID_INDEXER).map_err(|e| {
            StorageError::io_error(format!("Failed to write id_indexer header: {}", e))
        })?;

        // Use the new serialize method for cleaner code
        let index_data = self.id_indexer.serialize();
        payload.extend_from_slice(&index_data);

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.id_indexer.len() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)
    }

    fn flush_columns(
        &self,
        path: &Path,
        columns: &crate::vertex::ColumnStore,
    ) -> StorageResult<()> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_COLUMNS).map_err(|e| {
            StorageError::io_error(format!("Failed to write columns header: {}", e))
        })?;

        let column_count = columns.column_count() as u32;
        payload.extend_from_slice(&column_count.to_le_bytes());

        for col in columns.columns() {
            let name_bytes = col.name.as_bytes();
            payload.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(name_bytes);

            if col.encoding_type() != EncodingType::None {
                payload.push(1u8);
                let mut meta_buf = Vec::new();
                col.encoding().serialize_meta(&mut meta_buf)?;
                let meta_len = meta_buf.len() as u32;
                payload.extend_from_slice(&meta_len.to_le_bytes());
                payload.extend_from_slice(&meta_buf);

                let stats = col.compute_stats()?;
                payload.push(1u8);
                let mut stats_buf = Vec::new();
                stats.serialize_meta(&mut stats_buf)?;
                payload.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                payload.extend_from_slice(&stats_buf);
            } else {
                payload.push(0u8);
                let (data, offsets, bitmap) = col.get_flush_data();

                let row_count = offsets
                    .len()
                    .max(if data.is_empty() { 0 } else { col.len() });
                payload.extend_from_slice(&(row_count as u32).to_le_bytes());

                payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
                payload.extend_from_slice(&data);

                let offsets_count = offsets.len() as u32;
                payload.extend_from_slice(&offsets_count.to_le_bytes());
                for &off in &offsets {
                    payload.extend_from_slice(&off.to_le_bytes());
                }

                if let Some(bitmap) = bitmap {
                    payload.push(1u8);
                    let bitmap_bytes = bitmap.as_raw_slice();
                    let bitmap_bit_len = bitmap.len() as u32;
                    payload.extend_from_slice(&bitmap_bit_len.to_le_bytes());
                    payload.extend_from_slice(&(bitmap_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(bitmap_bytes);
                } else {
                    payload.push(0u8);
                }

                let stats = col.compute_stats()?;
                payload.push(1u8);
                let mut stats_buf = Vec::new();
                stats.serialize_meta(&mut stats_buf)?;
                payload.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                payload.extend_from_slice(&stats_buf);
            }
        }

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.columns.row_count() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)
    }

    fn flush_timestamps(&self, path: &Path) -> StorageResult<()> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_TIMESTAMPS).map_err(|e| {
            StorageError::io_error(format!("Failed to write timestamps header: {}", e))
        })?;

        let timestamps = self.timestamps.dump();
        let count = timestamps.len() as u32;
        payload.extend_from_slice(&count.to_le_bytes());

        for ts in timestamps {
            payload.extend_from_slice(&ts.to_le_bytes());
        }

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        Self::write_pages_to_file(path, &payload, page_size, 3, count)
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_internal(path)
    }

    fn read_pages_from_file(path: &Path) -> StorageResult<(Vec<u8>, u32)> {
        let file = std::fs::File::open(path).map_err(|e| {
            StorageError::io_error(format!("Failed to open {}: {}", path.display(), e))
        })?;
        let mut reader = std::io::BufReader::new(file);
        let header = crate::compression::ColumnFileHeader::deserialize(&mut reader)?;
        let total_rows = header.total_rows;
        let page_reader = crate::compression::PageReader::new(header.page_size);
        let data = page_reader.read_all(&mut reader, header.page_count)?;
        let mut trailing = [0u8; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(StorageError::deserialize_error(
                "trailing bytes after column file pages",
            ));
        }
        Ok((data, total_rows))
    }

    fn load_internal<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let (meta_data, _meta_rows) = Self::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = read_header(&mut slice)?;
            if sid != section::VERTEX_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex meta: expected {:#06x}, got {:#06x}",
                    section::VERTEX_META,
                    sid
                )));
            }
        }

        let mut label_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut label_bytes)?;
        self.label = u32::from_le_bytes(label_bytes);

        let mut label_name_len_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut label_name_len_bytes)?;
        let label_name_bytes = take_bytes(
            &mut meta_cursor,
            u32::from_le_bytes(label_name_len_bytes),
            "vertex label name",
        )?;
        self.label_name = String::from_utf8(label_name_bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

        let mut schema_len_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut schema_len_bytes)?;
        let schema_bytes = take_bytes(
            &mut meta_cursor,
            u32::from_le_bytes(schema_len_bytes),
            "vertex schema",
        )?;
        let schema_json = String::from_utf8(schema_bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        let schema: crate::vertex::VertexSchema = serde_json::from_str(&schema_json)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        self.set_schema(schema);
        if !meta_cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "trailing bytes in vertex metadata",
            ));
        }

        let id_indexer_path = path.join("id_indexer.bin");
        self.load_id_indexer(&id_indexer_path)?;

        let columns_path = path.join("columns.bin");
        self.load_columns(&columns_path)?;

        let timestamps_path = path.join("timestamps.bin");
        self.load_timestamps(&timestamps_path)?;

        self.is_open = true;
        Ok(())
    }

    pub(crate) fn load_id_indexer(&mut self, path: &Path) -> StorageResult<()> {
        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = read_header(&mut slice)?;
            if sid != section::VERTEX_ID_INDEXER {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex id_indexer: expected {:#06x}, got {:#06x}",
                    section::VERTEX_ID_INDEXER,
                    sid
                )));
            }
        }

        // Use the new deserialize method
        let remaining = cursor;
        let indexer = crate::vertex::IdIndexer::deserialize(remaining)?;
        self.id_indexer = indexer;

        // Verify total_rows
        if total_rows != self.id_indexer.len() as u32 {
            return Err(StorageError::deserialize_error(format!(
                "id_indexer total_rows mismatch: header={}, actual={}",
                total_rows,
                self.id_indexer.len()
            )));
        }

        Ok(())
    }

    fn load_columns(&mut self, path: &Path) -> StorageResult<()> {
        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = read_header(&mut slice)?;
            if sid != section::VERTEX_COLUMNS {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex columns: expected {:#06x}, got {:#06x}",
                    section::VERTEX_COLUMNS,
                    sid
                )));
            }
        }

        let mut column_count_bytes = [0u8; 4];
        cursor.read_exact(&mut column_count_bytes)?;
        let column_count = u32::from_le_bytes(column_count_bytes) as usize;

        self.columns.clear();

        for _ in 0..column_count {
            let mut name_len_bytes = [0u8; 4];
            cursor.read_exact(&mut name_len_bytes)?;
            let name_len = u32::from_le_bytes(name_len_bytes) as usize;

            let mut name_bytes = vec![0u8; name_len];
            cursor.read_exact(&mut name_bytes)?;
            let name = String::from_utf8(name_bytes)
                .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

            let mut has_encoding_bytes = [0u8; 1];
            cursor.read_exact(&mut has_encoding_bytes)?;
            let has_encoding = has_encoding_bytes[0] == 1;

            if has_encoding {
                let mut meta_len_bytes = [0u8; 4];
                cursor.read_exact(&mut meta_len_bytes)?;
                let meta_len = u32::from_le_bytes(meta_len_bytes) as usize;
                let mut meta_bytes = vec![0u8; meta_len];
                cursor.read_exact(&mut meta_bytes)?;
                let encoding_type = EncodingType::from_u8(meta_bytes[0]);
                let mut meta_cursor = &meta_bytes[1..];
                self.load_column_with_encoding(&name, encoding_type, &mut meta_cursor)?;

                let mut has_stats_bytes = [0u8; 1];
                cursor.read_exact(&mut has_stats_bytes)?;
                if has_stats_bytes[0] == 1 {
                    let mut stats_len_bytes = [0u8; 4];
                    cursor.read_exact(&mut stats_len_bytes)?;
                    let stats_len = u32::from_le_bytes(stats_len_bytes) as usize;
                    let mut stats_bytes = vec![0u8; stats_len];
                    cursor.read_exact(&mut stats_bytes)?;
                    let stats =
                        crate::column_stats::ColumnStats::deserialize_meta(&mut &stats_bytes[..])?;
                    if let Some(col) = self.columns.get_column_mut(&name) {
                        col.set_stats(stats);
                    }
                }
            } else {
                let mut row_count_bytes = [0u8; 4];
                cursor.read_exact(&mut row_count_bytes)?;
                let _row_count = u32::from_le_bytes(row_count_bytes) as usize;

                let mut data_len_bytes = [0u8; 4];
                cursor.read_exact(&mut data_len_bytes)?;
                let data_len = u32::from_le_bytes(data_len_bytes) as usize;

                let mut data = vec![0u8; data_len];
                cursor.read_exact(&mut data)?;

                let mut offsets_count_bytes = [0u8; 4];
                cursor.read_exact(&mut offsets_count_bytes)?;
                let offsets_count = u32::from_le_bytes(offsets_count_bytes) as usize;

                let mut offsets = Vec::with_capacity(offsets_count);
                for _ in 0..offsets_count {
                    let mut off_bytes = [0u8; 8];
                    cursor.read_exact(&mut off_bytes)?;
                    offsets.push(u64::from_le_bytes(off_bytes));
                }

                let mut has_bitmap_bytes = [0u8; 1];
                cursor.read_exact(&mut has_bitmap_bytes)?;
                let has_bitmap = has_bitmap_bytes[0] == 1;

                let (null_bitmap_raw, bitmap_bit_len) = if has_bitmap {
                    let mut bitmap_bit_len_bytes = [0u8; 4];
                    cursor.read_exact(&mut bitmap_bit_len_bytes)?;
                    let bitmap_bit_len = u32::from_le_bytes(bitmap_bit_len_bytes) as usize;

                    let mut bitmap_bytes_len_bytes = [0u8; 4];
                    cursor.read_exact(&mut bitmap_bytes_len_bytes)?;
                    let bitmap_bytes_len = u32::from_le_bytes(bitmap_bytes_len_bytes) as usize;

                    let mut bitmap_bytes = vec![0u8; bitmap_bytes_len];
                    cursor.read_exact(&mut bitmap_bytes)?;

                    (Some(bitmap_bytes), bitmap_bit_len)
                } else {
                    (None, 0)
                };

                self.columns.load_column_from_raw(
                    &name,
                    data,
                    offsets,
                    null_bitmap_raw,
                    bitmap_bit_len,
                )?;

                let mut has_stats_bytes = [0u8; 1];
                cursor.read_exact(&mut has_stats_bytes)?;
                if has_stats_bytes[0] == 1 {
                    let mut stats_len_bytes = [0u8; 4];
                    cursor.read_exact(&mut stats_len_bytes)?;
                    let stats_len = u32::from_le_bytes(stats_len_bytes) as usize;
                    let mut stats_bytes = vec![0u8; stats_len];
                    cursor.read_exact(&mut stats_bytes)?;
                    let stats =
                        crate::column_stats::ColumnStats::deserialize_meta(&mut &stats_bytes[..])?;
                    if let Some(col) = self.columns.get_column_mut(&name) {
                        col.set_stats(stats);
                    }
                }
            }
        }

        if total_rows > 0 && total_rows != self.columns.row_count() as u32 {
            return Err(StorageError::deserialize_error(format!(
                "columns total_rows mismatch: header={}, actual={}",
                total_rows,
                self.columns.row_count()
            )));
        }

        Ok(())
    }

    fn load_column_with_encoding(
        &mut self,
        name: &str,
        encoding_type: EncodingType,
        meta_cursor: &mut &[u8],
    ) -> StorageResult<()> {
        use crate::encoding::{
            AlpColumn, BitPackedIntColumn, ConstantColumn, DictionaryColumn, FsstColumn,
            RleBoolColumn, RleIntColumn,
        };

        match encoding_type {
            EncodingType::Fsst => {
                let col = FsstColumn::deserialize_meta(meta_cursor)?;
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                column.apply_fsst_from_meta(col)?;
            }
            EncodingType::Dictionary => {
                let col = DictionaryColumn::deserialize_meta(meta_cursor)?;
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                column.apply_dictionary_from_meta(col)?;
            }
            EncodingType::Rle => {
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                if column.data_type == graphdb_core::DataType::Bool {
                    let col = RleBoolColumn::deserialize_meta(meta_cursor)?;
                    column.apply_rle_bool_from_meta(col)?;
                } else {
                    let col = RleIntColumn::deserialize_meta(meta_cursor)?;
                    column.apply_rle_int_from_meta(col)?;
                }
            }
            EncodingType::BitPacking => {
                let col = BitPackedIntColumn::deserialize_meta(meta_cursor)?;
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                column.apply_bitpacked_from_meta(col)?;
            }
            EncodingType::Alp => {
                let col = AlpColumn::deserialize_meta(meta_cursor)?;
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                column.apply_alp_from_meta(col)?;
            }
            EncodingType::Constant => {
                let col = ConstantColumn::deserialize_meta(meta_cursor)?;
                let Some(column) = self.columns.get_column_mut(name) else {
                    return Err(StorageError::column_not_found(name.to_string()));
                };
                column.apply_constant_from_meta(col)?;
            }
            EncodingType::None => {}
        }
        Ok(())
    }

    pub(crate) fn load_timestamps(&mut self, path: &Path) -> StorageResult<()> {
        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = read_header(&mut slice)?;
            if sid != section::VERTEX_TIMESTAMPS {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex timestamps: expected {:#06x}, got {:#06x}",
                    section::VERTEX_TIMESTAMPS,
                    sid
                )));
            }
        }

        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        if total_rows > 0 && total_rows != count as u32 {
            return Err(StorageError::deserialize_error(format!(
                "timestamps total_rows mismatch: header={}, actual={}",
                total_rows, count
            )));
        }

        let mut timestamps = Vec::with_capacity(count);
        for _ in 0..count {
            let mut ts_bytes = [0u8; 8];
            cursor.read_exact(&mut ts_bytes)?;
            timestamps.push(u64::from_le_bytes(ts_bytes));
        }

        self.timestamps.load(&timestamps);

        self.is_open = true;
        Ok(())
    }

    /// Apply delta pages from an incremental checkpoint shard directory.
    pub fn apply_delta_pages(&mut self, shard_dir: &Path) -> StorageResult<()> {
        let delta_dir = shard_dir.join("columns_pages");
        if !delta_dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&delta_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("page") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            // Extract column name and page id from file name: "<col>_<page>.page"
            // We rely on deserialize to place rows correctly.
            // Try to deserialize into the appropriate column.
            // If file name doesn't match any column, skip.
            if let Some(file_name) = path.file_stem().and_then(|n| n.to_str()) {
                if let Some((col_name, _page_str)) = file_name.rsplit_once('_') {
                    if self.columns.get_column(col_name).is_some() {
                        if let Some(col) = self.columns.get_column_mut(col_name) {
                            if let Err(e) = col.deserialize_page(&bytes) {
                                log::warn!(
                                    "Skipping corrupted delta page {} for column {}: {}",
                                    path.display(),
                                    col_name,
                                    e
                                );
                            }
                            continue;
                        }
                    }
                }
            }
            // Fallback: try each column (covers naming mismatches)
            let mut applied = false;
            for col in self
                .columns
                .columns()
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
            {
                if let Some(c) = self.columns.get_column_mut(&col) {
                    if c.deserialize_page(&bytes).is_ok() {
                        applied = true;
                        break;
                    }
                }
            }
            if !applied {
                log::warn!(
                    "Skipping unrecognized or corrupted delta page {}",
                    path.display()
                );
            }
        }
        // After applying deltas, clear dirty marks (data now reflects persisted delta).
        self.clear_dirty();
        Ok(())
    }
}
