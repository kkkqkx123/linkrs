use std::path::Path;

use crate::compression::CompressionType;
use crate::encoding::EncodingType;
use crate::persistence::{section, write_header_to};
use graphdb_core::{StorageError, StorageResult};

use super::super::core::VertexTable;
use super::encoding_select::{select_encoding_for_column, COLUMNS_FORMAT_VERSION};

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
                            // Encoding selection already re-evaluates every
                            // chunk on each flush; this feedback only reports
                            // that the recent average ratio for this
                            // encoding/family stays above threshold.
                            log::debug!(
                                "column={} encoding={:?} avg_ratio={:.2} exceeds threshold, \
                                 future flushes keep re-evaluating selection",
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

    pub(super) fn build_meta_payload(&self) -> StorageResult<Vec<u8>> {
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

    pub(super) fn flush_id_indexer(&self, path: &Path) -> StorageResult<()> {
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

    pub(super) fn flush_timestamps(&self, path: &Path) -> StorageResult<()> {
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
}
