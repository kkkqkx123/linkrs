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

/// Columns file record-layout version. Development builds keep this at 1;
/// there is no backward compatibility with any other layout.
pub const COLUMNS_FORMAT_VERSION: u8 = 1;

/// Pick one encoding for a column by profiling each chunk independently
/// (streaming, no column-wide value vector) and voting for the most common
/// non-None chunk choice. Hot chunks vote `None` through the profile.
fn select_encoding_for_column(
    col: &crate::vertex::column::Column,
    selector: &crate::encoding::EncodingSelector,
) -> crate::encoding::EncodingType {
    use crate::encoding::{profile_chunk, EncodingType};
    use std::collections::HashMap;
    if col.is_empty() {
        return EncodingType::None;
    }
    let capacity = col.chunk_capacity().max(1);
    let total = col.len();
    let n_chunks = total.div_ceil(capacity).max(1);
    let mut votes: HashMap<u8, usize> = HashMap::new();
    for ci in 0..n_chunks {
        let start = ci * capacity;
        let end = (start + capacity).min(total);
        let hot = col
            .chunk_for_row(start)
            .map(|c| c.needs_recode(selector.thresholds().hot_update_threshold))
            .unwrap_or(false);
        let profile = profile_chunk((start..end).map(|r| col.get(r)), &col.data_type, hot);
        let choice = selector.select_for_chunk_profile(&profile);
        *votes.entry(choice.to_u8()).or_insert(0) += 1;
    }
    let (best_tag, best_count) = votes
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(tag, count)| (*tag, *count))
        .unwrap_or((EncodingType::None.to_u8(), 0));
    if best_tag == EncodingType::None.to_u8() || best_count == 0 {
        return EncodingType::None;
    }
    // Require a majority for non-trivial encodings on multi-chunk columns so
    // a single odd chunk cannot force a column-wide encoding.
    if n_chunks > 1 && best_count * 2 <= n_chunks {
        return EncodingType::None;
    }
    EncodingType::from_u8(best_tag)
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
        // Selection is streaming per chunk: each chunk profiles its own
        // rows without materializing a column-wide value vector.
        let selections = columns
            .columns()
            .iter()
            .map(|col| {
                let selection = select_encoding_for_column(col, &self.encoding_selector);
                (col.name.clone(), selection)
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
                        let family: crate::encoding::DataTypeFamily =
                            crate::encoding::data_type_family(&col.data_type);
                        self.encoding_selector.record_compression_result_for(
                            *encoding_type,
                            family,
                            stats.compression_ratio(),
                        );
                        if self
                            .encoding_selector
                            .should_reencode_for(*encoding_type, family)
                        {
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
        self.flush_columns(&columns_path, &mut columns)?;

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
        // Write per-column chunk metadata alongside columns.bin so reload
        // can reconstruct lazy-loaded segments without changing the format.
        self.flush_chunk_metadata(path)?;
        // Successful full flush clears dirty tracking (data now persisted).
        self.clear_dirty();

        Ok(())
    }

    /// Write per-column chunk metadata as `{col_name}.chunks` sidecar files.
    fn flush_chunk_metadata(&self, dir: &Path) -> StorageResult<()> {
        for col in self.columns.columns() {
            let chunk_meta: Vec<(usize, usize, u8)> = if col.has_chunks() {
                col.chunk_encoding_metadata()
                    .into_iter()
                    .map(|(idx, enc, rows)| (idx, rows, enc.to_u8()))
                    .collect()
            } else {
                vec![(0, col.len(), col.encoding_type().to_u8())]
            };
            let mut buf = Vec::new();
            buf.extend_from_slice(&(chunk_meta.len() as u32).to_le_bytes());
            for (idx, rows, enc) in &chunk_meta {
                buf.extend_from_slice(&(*idx as u32).to_le_bytes());
                buf.extend_from_slice(&(*rows as u32).to_le_bytes());
                buf.push(*enc);
            }
            let file_name = format!("{}.chunks", col.name);
            crate::compression::write_shadow_file(dir.join(file_name), &buf)?;
        }
        Ok(())
    }

    /// Load of chunk sidecars: materializes segments from the `{col}.chunks`
    /// metadata written by flush. Columns restored from chunk records in
    /// `columns.bin` already carry authoritative chunk state (encodings,
    /// overlays, rebuilt profiles) and are left untouched; the sidecar only
    /// fills the gap for raw columns.
    fn load_chunk_metadata(&mut self, dir: &Path) {
        for col_name in self
            .columns
            .columns()
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
        {
            let sidecar = dir.join(format!("{}.chunks", col_name));
            if !sidecar.exists() {
                continue;
            }
            if let Some(col) = self.columns.get_column_mut(&col_name) {
                if col.has_chunks() {
                    continue;
                }
                col.materialize_chunks();
                col.rebuild_chunk_profiles();
            }
        }
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

        let id_indexer_path = path.join("id_indexer.bin");
        self.flush_id_indexer(&id_indexer_path)?;

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

        Ok(tasks
            .into_iter()
            .map(|(name, pid, _)| (name, pid))
            .collect())
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
        columns: &mut crate::vertex::ColumnStore,
    ) -> StorageResult<()> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_COLUMNS).map_err(|e| {
            StorageError::io_error(format!("Failed to write columns header: {}", e))
        })?;

        let column_count = columns.column_count() as u32;
        payload.extend_from_slice(&column_count.to_le_bytes());
        payload.push(COLUMNS_FORMAT_VERSION);

        // Rebuild overflow sidecars from live rows so deleted payloads shrink.
        for col in columns.columns_mut() {
            col.rebuild_overflow();
        }

        let col_names: Vec<String> = columns.columns().iter().map(|c| c.name.clone()).collect();
        for name in &col_names {
            let col = columns.get_column(name).unwrap();
            let name_bytes = col.name.as_bytes();
            payload.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(name_bytes);

            if col.encoding_type() != EncodingType::None {
                payload.push(1u8);
                payload.push(1u8);
                // Ensure per-chunk segmentation exists on the snapshot.
                let col_mut = columns.get_column_mut(name).unwrap();
                if !col_mut.has_chunks() {
                    col_mut.materialize_chunks();
                }
                let col_ref = columns.get_column(name).unwrap();
                let chunk_count = col_ref.chunk_count().max(1) as u32;
                payload.extend_from_slice(&chunk_count.to_le_bytes());
                for ci in 0..col_ref.chunk_count().max(1) {
                    let chunk = col_ref.chunk_at(ci);
                    let (row_offset, row_count) = match chunk {
                        Some(c) => (c.row_offset as u32, c.row_count as u32),
                        None => (0u32, col_ref.len() as u32),
                    };
                    payload.extend_from_slice(&row_offset.to_le_bytes());
                    payload.extend_from_slice(&row_count.to_le_bytes());
                    // Compression metadata.
                    let mut meta_buf = Vec::new();
                    if let Some(c) = chunk {
                        c.encoding_meta.serialize(&mut meta_buf)?;
                    }
                    payload.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&meta_buf);
                    // Full encoding metadata for this chunk.
                    let mut enc_buf = Vec::new();
                    if let Some(c) = chunk {
                        c.encoding.serialize_meta(&mut enc_buf)?;
                    } else {
                        col_ref.encoding().serialize_meta(&mut enc_buf)?;
                    }
                    payload.extend_from_slice(&(enc_buf.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&enc_buf);
                    // Overlay entries.
                    let overlay_entries: Vec<(u32, Option<graphdb_core::Value>)> = chunk
                        .map(|c| c.overlay.iter().map(|(k, v)| (*k, v.clone())).collect())
                        .unwrap_or_default();
                    let overlay_bytes = postcard::to_allocvec(&overlay_entries)
                        .map_err(|e| StorageError::serialize_error(e.to_string()))?;
                    payload.extend_from_slice(&(overlay_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&overlay_bytes);
                }

                // Overflow sidecar presence flag (same semantics as raw).
                let overflow_present = (col_ref.has_overflow()
                    && matches!(
                        col_ref.data_type,
                        graphdb_core::DataType::String | graphdb_core::DataType::Blob
                    )) as u8;
                payload.push(overflow_present);

                Self::write_stats_with_fallback(&mut payload, col_ref);
            } else {
                payload.push(0u8);
                let col_ref = columns.get_column(name).unwrap();
                let (data, offsets, bitmap) = col_ref.get_flush_data();

                let row_count = offsets
                    .len()
                    .max(if data.is_empty() { 0 } else { col_ref.len() });
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

                // Overflow sidecar presence flag.
                let overflow_present = (col_ref.has_overflow()
                    && matches!(
                        col_ref.data_type,
                        graphdb_core::DataType::String | graphdb_core::DataType::Blob
                    )) as u8;
                payload.push(overflow_present);

                Self::write_stats_with_fallback(&mut payload, col_ref);
            }
        }

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.columns.row_count() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)?;

        // Persist overflow sidecars next to columns.bin.
        if let Some(dir) = path.parent() {
            for name in &col_names {
                let col = columns.get_column(name).unwrap();
                if !matches!(
                    col.data_type,
                    graphdb_core::DataType::String | graphdb_core::DataType::Blob
                ) {
                    continue;
                }
                if !col.has_overflow() {
                    continue;
                }
                match col.serialize_overflow() {
                    Ok(bytes) => {
                        let sidecar = dir.join(format!("{}.overflow", name));
                        if let Err(e) = crate::compression::write_shadow_file(&sidecar, &bytes) {
                            log::warn!("failed to write overflow sidecar for {}: {}", name, e);
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to serialize overflow for {}: {}", name, e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Serialize column stats, degrading to `has_stats=0` for composite
    /// columns whose bounds cannot be represented instead of failing flush.
    fn write_stats_with_fallback(payload: &mut Vec<u8>, col: &crate::vertex::column::Column) {
        match col.compute_stats() {
            Ok(stats) => match stats.serialize_meta(&mut Vec::new()) {
                Ok(_) => {
                    let mut stats_buf = Vec::new();
                    // Re-serialize into the real buffer (checked above).
                    let _ = stats.serialize_meta(&mut stats_buf);
                    payload.push(1u8);
                    payload.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&stats_buf);
                }
                Err(e) => {
                    log::warn!(
                        "column {} stats not representable, skipping stats: {}",
                        col.name,
                        e
                    );
                    payload.push(0u8);
                }
            },
            Err(e) => {
                log::warn!("column {} stats failed, skipping stats: {}", col.name, e);
                payload.push(0u8);
            }
        }
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
        // Reconstruct lazy-loaded chunk segments from sidecars when present.
        self.load_chunk_metadata(path);

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

        let mut ver = [0u8; 1];
        cursor.read_exact(&mut ver)?;
        if ver[0] != COLUMNS_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported columns format version {}",
                ver[0]
            )));
        }

        self.columns.clear();
        let dir = path.parent().map(|p| p.to_path_buf());

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
                let overflow_present = self.load_column_chunked(&name, &mut cursor)?;
                if overflow_present {
                    Self::load_overflow_sidecar(&mut self.columns, dir.as_deref(), &name);
                }
                // Chunk records carry encoding bodies and overlays, but the
                // derived per-chunk profiles (zone min/max, sizes) are
                // recomputed from the restored state so later selection and
                // update checks observe fresh metadata.
                if let Some(col) = self.columns.get_column_mut(&name) {
                    if col.has_chunks() {
                        col.rebuild_chunk_profiles();
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

                // Raw records carry an overflow sidecar flag.
                let mut flag = [0u8; 1];
                cursor.read_exact(&mut flag)?;
                let overflow_present = flag[0] != 0;

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

                if overflow_present {
                    Self::load_overflow_sidecar(&mut self.columns, dir.as_deref(), &name);
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

    /// Decode one chunked encoded-column record.
    /// Load one chunked column record. Returns whether an overflow sidecar
    /// must be restored afterwards (the caller owns the flush directory).
    fn load_column_chunked(&mut self, name: &str, cursor: &mut &[u8]) -> StorageResult<bool> {
        use crate::vertex::column::{element_size, is_variable_length_type, ColumnChunk};
        use graphdb_core::Value;

        let mut flag = [0u8; 1];
        cursor.read_exact(&mut flag)?;
        if flag[0] != 1 {
            return Err(StorageError::deserialize_error(format!(
                "unsupported chunked column marker {}",
                flag[0]
            )));
        }

        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let chunk_count = u32::from_le_bytes(count_bytes) as usize;

        struct ChunkRec {
            row_offset: usize,
            row_count: usize,
            meta: crate::encoding::ChunkEncodingMeta,
            encoding: crate::encoding::ColumnEncoding,
            overlay: Vec<(u32, Option<Value>)>,
        }
        let mut recs = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let mut u32b = [0u8; 4];
            cursor.read_exact(&mut u32b)?;
            let row_offset = u32::from_le_bytes(u32b) as usize;
            cursor.read_exact(&mut u32b)?;
            let row_count = u32::from_le_bytes(u32b) as usize;
            cursor.read_exact(&mut u32b)?;
            let meta_len = u32::from_le_bytes(u32b) as usize;
            let meta_bytes = take_bytes(cursor, meta_len as u32, "chunk meta")?;
            let meta = if meta_bytes.is_empty() {
                crate::encoding::ChunkEncodingMeta::default()
            } else {
                crate::encoding::ChunkEncodingMeta::deserialize(&mut &meta_bytes[..])?
            };
            cursor.read_exact(&mut u32b)?;
            let enc_len = u32::from_le_bytes(u32b) as usize;
            let enc_bytes = take_bytes(cursor, enc_len as u32, "chunk encoding")?;
            if enc_bytes.is_empty() {
                return Err(StorageError::deserialize_error(
                    "empty chunk encoding".to_string(),
                ));
            }
            let encoding_type = EncodingType::from_u8(enc_bytes[0]);
            let mut enc_cursor = &enc_bytes[1..];
            let chunk_data_type = self
                .columns
                .get_column(name)
                .map(|c| c.data_type.clone())
                .unwrap_or(graphdb_core::DataType::Int);
            let encoding = Self::decode_encoding(encoding_type, &mut enc_cursor, &chunk_data_type)?;
            cursor.read_exact(&mut u32b)?;
            let overlay_len = u32::from_le_bytes(u32b) as usize;
            let overlay_bytes = take_bytes(cursor, overlay_len as u32, "chunk overlay")?;
            let overlay: Vec<(u32, Option<Value>)> = if overlay_bytes.is_empty() {
                Vec::new()
            } else {
                postcard::from_bytes(&overlay_bytes)
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?
            };
            recs.push(ChunkRec {
                row_offset,
                row_count,
                meta,
                encoding,
                overlay,
            });
        }

        // Rebuild column state from chunk records.
        let total_rows = recs
            .iter()
            .map(|r| r.row_offset + r.row_count)
            .max()
            .unwrap_or(0);
        let col = self
            .columns
            .get_column_mut(name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        col.resize(total_rows);
        let data_type = col.data_type.clone();
        let nullable = col.nullable;
        let elem_size = element_size(&data_type);
        let is_var = is_variable_length_type(&data_type);
        let mut chunks = Vec::with_capacity(recs.len());
        for rec in &recs {
            let mut chunk = if is_var {
                ColumnChunk::new_variable(rec.row_offset, rec.row_count, nullable)
            } else {
                ColumnChunk::new(rec.row_offset, rec.row_count, elem_size, nullable)
            };
            chunk.encoding = rec.encoding.clone();
            chunk.encoding_meta = rec.meta.clone();
            for (local, v) in &rec.overlay {
                chunk.overlay.put(*local, v.clone());
            }
            chunk.updates_since_encode = chunk.overlay.len() as u64;
            chunks.push(chunk);
        }
        let first_encoding = recs.first().map(|r| r.encoding.clone());
        col.set_chunks(chunks);
        if let Some(enc) = first_encoding {
            col.restore_encoding(enc);
        }
        // Overflow flag sits between the chunk records and the stats suffix.
        let mut flag = [0u8; 1];
        cursor.read_exact(&mut flag)?;
        let overflow_present = flag[0] != 0;
        Self::load_stats_suffix(&mut self.columns, name, cursor)?;
        Ok(overflow_present)
    }

    /// Best-effort restore of a `<col>.overflow` sidecar: a corrupt or
    /// missing sidecar degrades to placeholder reads, never a load failure.
    fn load_overflow_sidecar(
        columns: &mut crate::vertex::ColumnStore,
        dir: Option<&std::path::Path>,
        name: &str,
    ) {
        let dir = match dir {
            Some(d) => d,
            None => return,
        };
        let sidecar = dir.join(format!("{}.overflow", name));
        if !sidecar.exists() {
            return;
        }
        match std::fs::read(&sidecar) {
            Ok(bytes) => {
                if let Some(col) = columns.get_column_mut(name) {
                    if let Err(e) = col.load_overflow_bytes(&bytes) {
                        log::warn!("ignoring corrupt overflow sidecar for {}: {}", name, e);
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to read overflow sidecar for {}: {}", name, e);
            }
        }
    }

    fn load_stats_suffix(
        columns: &mut crate::vertex::ColumnStore,
        name: &str,
        cursor: &mut &[u8],
    ) -> StorageResult<()> {
        let mut has_stats_bytes = [0u8; 1];
        cursor.read_exact(&mut has_stats_bytes)?;
        if has_stats_bytes[0] == 1 {
            let mut stats_len_bytes = [0u8; 4];
            cursor.read_exact(&mut stats_len_bytes)?;
            let stats_len = u32::from_le_bytes(stats_len_bytes) as usize;
            let mut stats_bytes = vec![0u8; stats_len];
            cursor.read_exact(&mut stats_bytes)?;
            let stats = crate::column_stats::ColumnStats::deserialize_meta(&mut &stats_bytes[..])?;
            if let Some(col) = columns.get_column_mut(name) {
                col.set_stats(stats);
            }
        }
        Ok(())
    }

    /// Decode one chunk encoding body.
    fn decode_encoding(
        encoding_type: EncodingType,
        meta_cursor: &mut &[u8],
        data_type: &graphdb_core::DataType,
    ) -> StorageResult<crate::encoding::ColumnEncoding> {
        use crate::encoding::{
            AlpColumn, BitPackedIntColumn, ColumnEncoding, ConstantColumn, DictionaryColumn,
            FsstColumn, RleBoolColumn, RleIntColumn,
        };
        match encoding_type {
            EncodingType::Fsst => Ok(ColumnEncoding::Fsst(FsstColumn::deserialize_meta(
                meta_cursor,
            )?)),
            EncodingType::Dictionary => Ok(ColumnEncoding::Dictionary(
                DictionaryColumn::deserialize_meta(meta_cursor)?,
            )),
            EncodingType::Rle => {
                if *data_type == graphdb_core::DataType::Bool {
                    Ok(ColumnEncoding::RleBool(RleBoolColumn::deserialize_meta(
                        meta_cursor,
                    )?))
                } else {
                    Ok(ColumnEncoding::RleInt(RleIntColumn::deserialize_meta(
                        meta_cursor,
                    )?))
                }
            }
            EncodingType::BitPacking => Ok(ColumnEncoding::BitPacked(
                BitPackedIntColumn::deserialize_meta(meta_cursor)?,
            )),
            EncodingType::Alp => Ok(ColumnEncoding::Alp(AlpColumn::deserialize_meta(
                meta_cursor,
            )?)),
            EncodingType::Constant => Ok(ColumnEncoding::Constant(
                ConstantColumn::deserialize_meta(meta_cursor)?,
            )),
            EncodingType::None => Ok(ColumnEncoding::None),
        }
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
        let mut applied: Vec<(String, usize)> = Vec::new();
        for entry in std::fs::read_dir(&delta_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("page") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            // File name encodes the target column: "<col>_<page>.page".
            // Only the named column may consume the page; a page that fails
            // to deserialize there is corrupt and must not be tried against
            // other columns (their payloads are independent).
            let (col_name, page_id) = match path
                .file_stem()
                .and_then(|n| n.to_str())
                .and_then(|stem| stem.rsplit_once('_'))
            {
                Some((name, page)) => match page.parse::<usize>() {
                    Ok(id) => (name.to_string(), id),
                    Err(_) => {
                        log::warn!("Skipping delta page with bad name {}", path.display());
                        continue;
                    }
                },
                None => {
                    log::warn!("Skipping delta page with bad name {}", path.display());
                    continue;
                }
            };
            let Some(col) = self.columns.get_column_mut(&col_name) else {
                log::warn!(
                    "Skipping delta page {} for unknown column {}",
                    path.display(),
                    col_name
                );
                continue;
            };
            if let Err(e) = col.deserialize_page(&bytes) {
                log::warn!(
                    "Skipping corrupted delta page {} for column {}: {}",
                    path.display(),
                    col_name,
                    e
                );
                continue;
            }
            applied.push((col_name, page_id));
        }
        // Mark only successfully applied pages clean; corrupt or unknown
        // pages stay dirty so the next flush retries them.
        self.columns.clear_pages(&applied);
        Ok(())
    }
}
