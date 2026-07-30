//! Vertex Table Persistence Layer
//!
//! Handles serialization, deserialization, and file I/O for vertex tables.
//!
//! # Encoding Handling
//! - Encodings are serialized as structured metadata during flush
//! - Encodings are reconstructed directly via `deserialize_meta()` on load

use std::io::Read;
use std::path::Path;

use crate::core::{StorageError, StorageResult};
use crate::storage::compression::CompressionType;
use crate::storage::encoding::EncodingType;
use crate::storage::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use crate::storage::vertex::IdKey;

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
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;

        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::storage::compression::cleanup_shadow_files(path)?;

        let CompressionType::Zstd { level } = compression;
        let page_size = crate::storage::compression::DEFAULT_PAGE_SIZE;

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
                                self.encoding_selector
                                    .thresholds()
                                    .reencode_threshold,
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
                    log::warn!("failed to apply encoding to in-memory column {}: {}", name, e);
                }
            }
        }

        let timestamps_path = path.join("timestamps.bin");
        self.flush_timestamps(&timestamps_path)?;

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
        let mut writer = crate::storage::compression::PageWriter::new(page_size, level);
        writer.write_all(&mut pages_buf, payload)?;

        let mut final_buf = Vec::new();
        let header = crate::storage::compression::ColumnFileHeader {
            page_size,
            page_count: writer.page_count(),
            total_rows,
        };
        header.serialize(&mut final_buf)?;
        final_buf.extend_from_slice(&pages_buf);

        crate::storage::compression::write_shadow_file(path, &final_buf)
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

        let count = self.id_indexer.len() as u32;
        payload.extend_from_slice(&count.to_le_bytes());

        let mut key_buf = Vec::new();
        for (key, id) in self.id_indexer.iter() {
            payload.extend_from_slice(&id.to_le_bytes());
            key.write_to(&mut key_buf);
            payload.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
            payload.extend_from_slice(&key_buf);
        }

        let page_size = crate::storage::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.id_indexer.len() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)
    }

    fn flush_columns(
        &self,
        path: &Path,
        columns: &crate::storage::vertex::ColumnStore,
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

        let page_size = crate::storage::compression::DEFAULT_PAGE_SIZE;
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

        let page_size = crate::storage::compression::DEFAULT_PAGE_SIZE;
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
        let header = crate::storage::compression::ColumnFileHeader::deserialize(&mut reader)?;
        let total_rows = header.total_rows;
        let page_reader = crate::storage::compression::PageReader::new(header.page_size);
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
        let schema: crate::storage::vertex::VertexSchema = serde_json::from_str(&schema_json)
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

    fn load_id_indexer(&mut self, path: &Path) -> StorageResult<()> {
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

        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        self.id_indexer.clear();

        for _ in 0..count {
            let mut id_bytes = [0u8; 4];
            cursor.read_exact(&mut id_bytes)?;
            let internal_id = u32::from_le_bytes(id_bytes);

            let mut key_len_bytes = [0u8; 4];
            cursor.read_exact(&mut key_len_bytes)?;
            let key_bytes = take_bytes(
                &mut cursor,
                u32::from_le_bytes(key_len_bytes),
                "vertex id key",
            )?;
            let key = IdKey::from_bytes(&key_bytes)?;

            self.id_indexer.set_at(internal_id, key);
        }

        if total_rows != count as u32 {
            return Err(StorageError::deserialize_error(format!(
                "id_indexer total_rows mismatch: header={}, actual={}",
                total_rows, count
            )));
        }
        if !cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "trailing bytes in vertex id index",
            ));
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
                    let stats = crate::storage::column_stats::ColumnStats::deserialize_meta(
                        &mut &stats_bytes[..],
                    )?;
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
                    let stats = crate::storage::column_stats::ColumnStats::deserialize_meta(
                        &mut &stats_bytes[..],
                    )?;
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
        use crate::storage::encoding::{
            AlpColumn, BitPackedIntColumn, DictionaryColumn, FsstColumn, RleBoolColumn,
            RleIntColumn,
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
                if column.data_type == crate::core::DataType::Bool {
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
            EncodingType::None => {}
        }
        Ok(())
    }

    fn load_timestamps(&mut self, path: &Path) -> StorageResult<()> {
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
}
