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

        self.timestamps.load(&timestamps);

        self.is_open = true;
        Ok(())
    }
}
