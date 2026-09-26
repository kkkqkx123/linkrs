use std::io::Read;
use std::path::Path;

use crate::encoding::EncodingType;
use crate::persistence::{read_header, section, HEADER_SIZE};
use graphdb_core::{StorageError, StorageResult};

use super::super::core::VertexTable;
use super::common::take_bytes;
use super::encoding_select::COLUMNS_FORMAT_VERSION;

impl VertexTable {
    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_internal(path)
    }

    pub(crate) fn read_pages_from_file(path: &Path) -> StorageResult<(Vec<u8>, u32)> {
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
            let sid = read_header(&mut slice)?;
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
        self.set_schema(schema)?;
        if !meta_cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "trailing bytes in vertex metadata",
            ));
        }

        let id_indexer_path = path.join("id_indexer.bin");
        self.load_id_indexer(&id_indexer_path)?;

        let columns_path = path.join("columns.bin");
        self.load_columns(&columns_path)?;
        // Restore persisted eviction state from mmap sidecars. Derived
        // cache only: failures keep chunks resident without failing load.
        // Discard counts are observable via the warn above; the manifest
        // pin (sharded layer) already pruned tampered sidecars.
        let _ = self.columns.load_evict_snapshots(path);

        let timestamps_path = path.join("timestamps.bin");
        self.load_timestamps(&timestamps_path)?;

        self.is_open
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub(crate) fn load_id_indexer(&mut self, path: &Path) -> StorageResult<()> {
        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = read_header(&mut slice)?;
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

    /// Overlay a since-baseline primary-key delta (`id_indexer.delta`)
    /// onto the loaded baseline. Returns the applied entry count.
    /// Corrupt entries fail the whole delta and refuse the open.
    /// Replay never extends the live delta log.
    pub(crate) fn load_id_indexer_delta(&mut self, path: &Path) -> StorageResult<usize> {
        let (data, _) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = read_header(&mut slice)?;
            if sid != section::VERTEX_ID_INDEXER_DELTA {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex id_indexer delta: expected {:#06x}, got {:#06x}",
                    section::VERTEX_ID_INDEXER_DELTA,
                    sid
                )));
            }
        }

        let remaining = cursor;
        let entries = crate::vertex::IdIndexer::deserialize_delta(remaining)?;
        let count = entries.len();
        self.id_indexer.apply_delta_entries(&entries)?;

        Ok(count)
    }

    /// Offline read-only verification of one shard's primary-key files.
    ///
    /// Returns issue strings, empty when the index is openable: the
    /// baseline decodes with a matching row count, the delta decodes, and
    /// the delta applies onto the baseline without divergence. A delta
    /// without a same-directory baseline only gets a format check (its
    /// anchor lives in the base checkpoint). Never writes.
    pub(crate) fn verify_pk_files(dir: &Path) -> Vec<String> {
        use crate::vertex::id_indexer::IdManager;

        let mut issues = Vec::new();
        let base_path = dir.join("id_indexer.bin");
        let delta_path = dir.join("id_indexer.delta");
        if !base_path.exists() && !delta_path.exists() {
            return issues;
        }
        let mut baseline: Option<IdManager> = None;
        if base_path.exists() {
            match Self::decode_pk_baseline(&base_path) {
                Ok(manager) => baseline = Some(manager),
                Err(e) => issues.push(format!(
                    "pk baseline corrupt at {}: {}",
                    base_path.display(),
                    e
                )),
            }
        }
        if delta_path.exists() {
            match Self::decode_pk_delta(&delta_path) {
                Ok(entries) => {
                    if let Some(mut manager) = baseline {
                        if base_path.exists() {
                            issues.push(format!(
                                "pk delta present alongside baseline at {}: superseded files must not linger",
                                delta_path.display(),
                            ));
                        }
                        if let Err(e) = manager.apply_delta_entries(&entries) {
                            issues.push(format!(
                                "pk delta diverges at {}: {}",
                                delta_path.display(),
                                e
                            ));
                        }
                    }
                }
                Err(e) => issues.push(format!(
                    "pk delta corrupt at {}: {}",
                    delta_path.display(),
                    e
                )),
            }
        }
        issues
    }

    fn decode_pk_baseline(path: &Path) -> StorageResult<crate::vertex::id_indexer::IdManager> {
        use crate::vertex::id_indexer::IdManager;

        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = read_header(&mut slice)?;
            if sid != section::VERTEX_ID_INDEXER {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex id_indexer: expected {:#06x}, got {:#06x}",
                    section::VERTEX_ID_INDEXER,
                    sid
                )));
            }
        }
        let manager = IdManager::deserialize(cursor)?;
        if total_rows != manager.len() as u32 {
            return Err(StorageError::deserialize_error(format!(
                "id_indexer total_rows mismatch: header={}, actual={}",
                total_rows,
                manager.len()
            )));
        }
        Ok(manager)
    }

    fn decode_pk_delta(path: &Path) -> StorageResult<Vec<(u8, u32, crate::vertex::IdKey)>> {
        let (data, _) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = read_header(&mut slice)?;
            if sid != section::VERTEX_ID_INDEXER_DELTA {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in vertex id_indexer delta: expected {:#06x}, got {:#06x}",
                    section::VERTEX_ID_INDEXER_DELTA,
                    sid
                )));
            }
        }
        crate::vertex::id_indexer::IdManager::deserialize_delta(cursor)
    }

    fn load_columns(&mut self, path: &Path) -> StorageResult<()> {
        let (data, total_rows) = Self::read_pages_from_file(path)?;
        let mut cursor = &data[..];
        let mut header_buf = [0u8; HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = read_header(&mut slice)?;
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
                "unsupported columns format version {} (expected {}): this data \
                 directory was written by an incompatible layout and cannot be \
                 opened; rebuild it by flushing from a compatible source",
                ver[0], COLUMNS_FORMAT_VERSION
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

            // One unified chunked record per column: each chunk carries
            // either its raw buffers or its encoding, followed by its
            // overlay; overflow and stats trail all records.
            let overflow_present = self.load_column_chunked(&name, &mut cursor)?;
            // Records carry payloads and overlays, but the derived per-chunk
            // profiles (zone min/max, sizes) are recomputed from the restored
            // state so later selection and update checks observe fresh
            // metadata.
            if let Some(col) = self.columns.get_column(&name) {
                col.rebuild_chunk_profiles();
            }
            if overflow_present {
                Self::load_overflow_sidecar(&mut self.columns, dir.as_deref(), &name);
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

    /// Load one chunked column record. Returns whether an overflow sidecar
    /// must be restored afterwards (the caller owns the flush directory).
    fn load_column_chunked(&mut self, name: &str, cursor: &mut &[u8]) -> StorageResult<bool> {
        use crate::vertex::column::ColumnChunk;
        use graphdb_core::Value;

        let col = self
            .columns
            .get_column(name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let data_type = col.data_type.clone();
        let nullable = col.nullable;

        let mut u32b = [0u8; 4];
        cursor.read_exact(&mut u32b)?;
        let chunk_count = u32::from_le_bytes(u32b) as usize;

        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            cursor.read_exact(&mut u32b)?;
            let row_offset = u32::from_le_bytes(u32b) as usize;
            cursor.read_exact(&mut u32b)?;
            let row_count = u32::from_le_bytes(u32b) as usize;
            let mut form = [0u8; 1];
            cursor.read_exact(&mut form)?;

            let chunk = ColumnChunk::new(row_offset, row_count, &data_type, nullable);
            let mut state = chunk.write_state();
            match form[0] {
                0 => {
                    cursor.read_exact(&mut u32b)?;
                    let data_len = u32::from_le_bytes(u32b) as usize;
                    let data = take_bytes(cursor, data_len as u32, "chunk raw data")?;
                    cursor.read_exact(&mut u32b)?;
                    let offsets_count = u32::from_le_bytes(u32b) as usize;
                    let mut offsets = Vec::with_capacity(offsets_count);
                    for _ in 0..offsets_count {
                        let mut off_bytes = [0u8; 8];
                        cursor.read_exact(&mut off_bytes)?;
                        offsets.push(u64::from_le_bytes(off_bytes));
                    }
                    let mut bitmap_flag = [0u8; 1];
                    cursor.read_exact(&mut bitmap_flag)?;
                    let (bitmap_raw, bit_len) = if bitmap_flag[0] == 1 {
                        cursor.read_exact(&mut u32b)?;
                        let bit_len = u32::from_le_bytes(u32b) as usize;
                        cursor.read_exact(&mut u32b)?;
                        let bytes_len = u32::from_le_bytes(u32b) as usize;
                        let bytes = take_bytes(cursor, bytes_len as u32, "chunk null bitmap")?;
                        (Some(bytes), bit_len)
                    } else {
                        (None, 0)
                    };
                    state
                        .raw
                        .as_storage_mut()
                        .load_data_from_raw(data, offsets, bitmap_raw, bit_len);
                }
                1 => {
                    cursor.read_exact(&mut u32b)?;
                    let meta_len = u32::from_le_bytes(u32b) as usize;
                    let meta_bytes = take_bytes(cursor, meta_len as u32, "chunk meta")?;
                    state.encoding_meta = if meta_bytes.is_empty() {
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
                    state.encoding =
                        Self::decode_encoding(encoding_type, &mut enc_cursor, &data_type)?;
                }
                other => {
                    return Err(StorageError::deserialize_error(format!(
                        "unsupported chunk form {}",
                        other
                    )));
                }
            }
            cursor.read_exact(&mut u32b)?;
            let overlay_len = u32::from_le_bytes(u32b) as usize;
            let overlay_bytes = take_bytes(cursor, overlay_len as u32, "chunk overlay")?;
            if !overlay_bytes.is_empty() {
                let overlay: Vec<(u32, Option<Value>)> = postcard::from_bytes(&overlay_bytes)
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
                for (local, v) in overlay {
                    state.overlay.put(local, v);
                }
            }
            state.updates_since_encode = state.overlay.len() as u64;
            // Restored windows carry no per-row MVCC deltas (checkpoints
            // persist current values), but the state slices must still span
            // the window so later splits and truncates stay in bounds.
            state.visibility.ensure_len(row_count);
            drop(state);
            chunks.push(chunk);
        }
        col.set_chunks(chunks);
        // Overflow flag sits between the chunk records and the stats suffix.
        let mut flag = [0u8; 1];
        cursor.read_exact(&mut flag)?;
        let overflow_present = flag[0] != 0;
        Self::load_stats_suffix(&self.columns, name, cursor)?;
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
                if let Some(col) = columns.get_column(name) {
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
        columns: &crate::vertex::ColumnStore,
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
            if let Some(col) = columns.get_column(name) {
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
            let sid = read_header(&mut slice)?;
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

        self.timestamps.write().load(&timestamps);

        self.is_open
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}
