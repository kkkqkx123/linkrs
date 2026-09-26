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
        self.flush_id_indexer_baseline(path, &id_indexer_path)?;

        let columns_path = path.join("columns.bin");
        // Encoding is a flush-time concern. Columns stream through one at a
        // time (clone, promote evicted chunks, select encoding, serialize,
        // drop) so the flush peak stays near one column instead of the whole
        // store. The live table stays evicted throughout; only each
        // per-column snapshot is promoted.
        let selections = self.flush_columns(&columns_path)?;

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
        // Persist evicted chunks into mmap sidecars so reload restores the
        // eviction state instead of decoding everything onto the heap.
        self.columns.flush_evict_snapshots(path)?;
        // Successful full flush clears dirty tracking (data now persisted).
        self.clear_dirty();

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

    /// Anchor a new primary-key baseline: full snapshot plus removal of any
    /// superseded `id_indexer.delta`, with the live delta log cleared.
    /// Full-flush path only.
    pub(super) fn flush_id_indexer_baseline(&self, dir: &Path, path: &Path) -> StorageResult<()> {
        self.flush_id_indexer(path)?;
        let delta_path = dir.join("id_indexer.delta");
        if delta_path.exists() {
            std::fs::remove_file(&delta_path)?;
        }
        self.id_indexer.clear_index_delta();
        Ok(())
    }

    /// Persist the since-baseline primary-key delta (`id_indexer.delta`).
    /// Skipped by the caller when the delta is empty; shares the checkpoint
    /// commit with the column delta pages.
    pub(super) fn flush_id_indexer_delta(&self, path: &Path) -> StorageResult<()> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_ID_INDEXER_DELTA).map_err(|e| {
            StorageError::io_error(format!("Failed to write id_indexer delta header: {}", e))
        })?;

        let delta_data = self.id_indexer.serialize_delta();
        payload.extend_from_slice(&delta_data);

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.id_indexer.delta_len() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)
    }

    /// Persist every column in one pass, streaming column by column.
    ///
    /// Each column is cloned, promoted, encoded, and serialized before the
    /// next one is touched, so the flush peak stays near a single column.
    /// Returns the per-column encoding selections for the in-memory
    /// post-flush encoding pass.
    fn flush_columns(
        &mut self,
        path: &Path,
    ) -> StorageResult<Vec<(String, crate::encoding::EncodingType)>> {
        let mut payload = Vec::new();
        write_header_to(&mut payload, section::VERTEX_COLUMNS).map_err(|e| {
            StorageError::io_error(format!("Failed to write columns header: {}", e))
        })?;

        let column_count = self.columns.column_count() as u32;
        payload.extend_from_slice(&column_count.to_le_bytes());
        payload.push(COLUMNS_FORMAT_VERSION);

        let col_names: Vec<String> = self.columns.column_names();
        let mut selections = Vec::with_capacity(col_names.len());
        for name in &col_names {
            let mut snapshot = self
                .columns
                .get_column(name)
                .map(|col| col.clone())
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            // Rebuild overflow sidecars from live rows so deleted payloads shrink.
            snapshot.rebuild_overflow();
            // Evicted chunks promote on the snapshot only: persisted output
            // keeps full fidelity while the live table stays evicted.
            match snapshot.ensure_all_resident() {
                Ok(0) => {}
                Ok(loaded) => {
                    log::debug!("flush promoted {} evicted chunks", loaded);
                }
                Err(e) => {
                    log::warn!("flush chunk promotion failed: {}", e);
                }
            }
            // Use the persistent encoding selector so compression feedback
            // accumulates across flushes, enabling the re-encoding detector.
            // Selection is streaming per chunk: each chunk profiles its own
            // rows without materializing a column-wide value vector.
            let selection = select_encoding_for_column(&snapshot, &self.encoding_selector);
            if selection != EncodingType::None {
                snapshot.apply_selected_encoding(
                    selection,
                    self.encoding_selector.thresholds().fsst_max_symbols,
                )?;
                if let Ok(stats) = snapshot.compute_stats() {
                    log::debug!(
                        "flush column={} encoding={:?} ratio={:.2}% savings={:.2}% raw={} compressed={}",
                        name,
                        selection,
                        stats.compression_ratio() * 100.0,
                        stats.space_savings() * 100.0,
                        stats.raw_size,
                        stats.compressed_size,
                    );
                    let family: crate::encoding::DataTypeFamily =
                        crate::encoding::data_type_family(&snapshot.data_type);
                    self.encoding_selector.record_compression_result_for(
                        selection,
                        family,
                        stats.compression_ratio(),
                    );
                    if self
                        .encoding_selector
                        .should_reencode_for(selection, family)
                    {
                        // Encoding selection already re-evaluates every
                        // chunk on each flush; this feedback only reports
                        // that the recent average ratio for this
                        // encoding/family stays above threshold.
                        log::debug!(
                            "column={} encoding={:?} avg_ratio={:.2} exceeds threshold, \
                             future flushes keep re-evaluating selection",
                            name,
                            selection,
                            self.encoding_selector.thresholds().reencode_threshold,
                        );
                    }
                }
            }
            Self::append_column_payload(&mut payload, &mut snapshot)?;
            if matches!(
                snapshot.data_type,
                graphdb_core::DataType::String | graphdb_core::DataType::Blob
            ) && snapshot.has_overflow()
            {
                match snapshot.serialize_overflow() {
                    Ok(bytes) => {
                        let sidecar = path
                            .parent()
                            .unwrap_or(Path::new("."))
                            .join(format!("{}.overflow", name));
                        if let Err(e) = crate::compression::write_shadow_file(&sidecar, &bytes) {
                            log::warn!("failed to write overflow sidecar for {}: {}", name, e);
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to serialize overflow for {}: {}", name, e);
                    }
                }
            }
            selections.push((name.clone(), selection));
        }

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        let total_rows = self.columns.row_count() as u32;
        Self::write_pages_to_file(path, &payload, page_size, 3, total_rows)?;

        Ok(selections)
    }

    /// Serialize one prepared column snapshot into the columns payload.
    /// Layout: name, chunk count, then one record per chunk carrying either
    /// its encoding (plus compression metadata) or its raw buffers, followed
    /// by the chunk overlay. Overflow and statistics trail all records.
    fn append_column_payload(
        payload: &mut Vec<u8>,
        col: &crate::vertex::column::Column,
    ) -> StorageResult<()> {
        let name_bytes = col.name.as_bytes();
        payload.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(name_bytes);

        let views = col.chunk_flush_views();
        payload.extend_from_slice(&(views.len() as u32).to_le_bytes());
        for view in &views {
            payload.extend_from_slice(&(view.row_offset as u32).to_le_bytes());
            payload.extend_from_slice(&(view.row_count as u32).to_le_bytes());
            if view.raw_form {
                payload.push(0u8);
                payload.extend_from_slice(&(view.raw_data.len() as u32).to_le_bytes());
                payload.extend_from_slice(&view.raw_data);
                payload.extend_from_slice(&(view.raw_offsets.len() as u32).to_le_bytes());
                for &off in &view.raw_offsets {
                    payload.extend_from_slice(&off.to_le_bytes());
                }
                match &view.raw_bitmap {
                    Some(bitmap) => {
                        payload.push(1u8);
                        let bitmap_bytes = bitmap.as_raw_slice();
                        payload.extend_from_slice(&(bitmap.len() as u32).to_le_bytes());
                        payload.extend_from_slice(&(bitmap_bytes.len() as u32).to_le_bytes());
                        payload.extend_from_slice(bitmap_bytes);
                    }
                    None => payload.push(0u8),
                }
            } else {
                payload.push(1u8);
                let mut meta_buf = Vec::new();
                view.encoding_meta.serialize(&mut meta_buf)?;
                payload.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
                payload.extend_from_slice(&meta_buf);
                let mut enc_buf = Vec::new();
                view.encoding.serialize_meta(&mut enc_buf)?;
                payload.extend_from_slice(&(enc_buf.len() as u32).to_le_bytes());
                payload.extend_from_slice(&enc_buf);
            }
            let overlay_bytes = postcard::to_allocvec(&view.overlay)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            payload.extend_from_slice(&(overlay_bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(&overlay_bytes);
        }

        // Overflow sidecar presence flag.
        let overflow_present = (col.has_overflow()
            && matches!(
                col.data_type,
                graphdb_core::DataType::String | graphdb_core::DataType::Blob
            )) as u8;
        payload.push(overflow_present);

        Self::write_stats_with_fallback(payload, col);
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

        let timestamps = self.timestamps.read().dump();
        let count = timestamps.len() as u32;
        payload.extend_from_slice(&count.to_le_bytes());

        for ts in timestamps {
            payload.extend_from_slice(&ts.to_le_bytes());
        }

        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        Self::write_pages_to_file(path, &payload, page_size, 3, count)
    }
}
