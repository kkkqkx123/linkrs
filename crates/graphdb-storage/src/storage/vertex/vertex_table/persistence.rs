//! Vertex Table Persistence Layer
//!
//! Handles serialization, deserialization, and file I/O for vertex tables.
//!
//! # Encoding Handling
//! - Deferred encodings are loaded and stored separately
//! - Can be applied eagerly via `ensure_encodings()` after load
//! - Preserves encoding metadata across flush/load cycles

use std::io::{Read, Write};
use std::path::Path;

use crate::core::{StorageError, StorageResult};
use crate::storage::encoding::EncodingType;
use crate::storage::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use crate::storage::vertex::IdKey;

use super::core::VertexTable;

impl VertexTable {
    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs::{self, File};

        // Warn if there are unapplied deferred encodings
        if !self.deferred_encodings.is_empty() {
            eprintln!(
                "WARNING: Flushing VertexTable with {} unapplied deferred encodings. \
                 Call ensure_encodings() before flush for better space efficiency.",
                self.deferred_encodings.len()
            );
        }

        let path = path.as_ref();
        fs::create_dir_all(path)?;

        let meta_path = path.join("meta.bin");
        let mut meta_file = File::create(&meta_path)?;
        write_header_to(&mut meta_file, section::VERTEX_META)
            .map_err(|e| StorageError::io_error(format!("Failed to write meta header: {}", e)))?;

        let label_bytes = self.label.to_le_bytes();
        let label_name_bytes = self.label_name.as_bytes();
        let label_name_len = label_name_bytes.len() as u32;

        meta_file.write_all(&label_bytes)?;
        meta_file.write_all(&label_name_len.to_le_bytes())?;
        meta_file.write_all(label_name_bytes)?;

        let schema_json = serde_json::to_string(&self.schema)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let schema_bytes = schema_json.as_bytes();
        meta_file.write_all(&(schema_bytes.len() as u32).to_le_bytes())?;
        meta_file.write_all(schema_bytes)?;

        drop(meta_file);
        crate::storage::compression::compress_file_inplace(&meta_path, compression)?;

        let id_indexer_path = path.join("id_indexer.bin");
        self.flush_id_indexer(&id_indexer_path)?;
        crate::storage::compression::compress_file_inplace(&id_indexer_path, compression)?;

        let columns_path = path.join("columns.bin");
        self.flush_columns(&columns_path)?;
        crate::storage::compression::compress_file_inplace(&columns_path, compression)?;

        let timestamps_path = path.join("timestamps.bin");
        self.flush_timestamps(&timestamps_path)?;
        crate::storage::compression::compress_file_inplace(&timestamps_path, compression)?;

        Ok(())
    }

    fn flush_id_indexer(&self, path: &Path) -> StorageResult<()> {
        use std::fs::File;

        let mut file = File::create(path)?;
        write_header_to(&mut file, section::VERTEX_ID_INDEXER).map_err(|e| {
            StorageError::io_error(format!("Failed to write id_indexer header: {}", e))
        })?;

        let count = self.id_indexer.len() as u32;
        file.write_all(&count.to_le_bytes())?;

        let mut key_buf = Vec::new();
        for (key, id) in self.id_indexer.iter() {
            file.write_all(&id.to_le_bytes())?;
            key.write_to(&mut key_buf);
            file.write_all(&(key_buf.len() as u32).to_le_bytes())?;
            file.write_all(&key_buf)?;
        }

        Ok(())
    }

    fn flush_columns(&self, path: &Path) -> StorageResult<()> {
        use std::fs::File;

        let mut file = File::create(path)?;
        write_header_to(&mut file, section::VERTEX_COLUMNS).map_err(|e| {
            StorageError::io_error(format!("Failed to write columns header: {}", e))
        })?;

        let column_count = self.columns.column_count() as u32;
        file.write_all(&column_count.to_le_bytes())?;

        for col in self.columns.columns() {
            let name_bytes = col.name.as_bytes();
            file.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
            file.write_all(name_bytes)?;

            let (data, offsets, bitmap) = col.get_flush_data();

            let row_count = offsets
                .len()
                .max(if data.is_empty() { 0 } else { col.len() });
            file.write_all(&(row_count as u32).to_le_bytes())?;

            file.write_all(&(data.len() as u32).to_le_bytes())?;
            file.write_all(&data)?;

            let offsets_count = offsets.len() as u32;
            file.write_all(&offsets_count.to_le_bytes())?;
            for &off in &offsets {
                file.write_all(&off.to_le_bytes())?;
            }

            if let Some(bitmap) = bitmap {
                file.write_all(&[1u8])?;
                let bitmap_bytes = bitmap.as_raw_slice();
                let bitmap_bit_len = bitmap.len() as u32;
                file.write_all(&bitmap_bit_len.to_le_bytes())?;
                file.write_all(&(bitmap_bytes.len() as u32).to_le_bytes())?;
                file.write_all(bitmap_bytes)?;
            } else {
                file.write_all(&[0u8])?;
            }

            let encoding_type = col.encoding_type().to_u8();
            file.write_all(&[encoding_type])?;
        }

        Ok(())
    }

    fn flush_timestamps(&self, path: &Path) -> StorageResult<()> {
        use std::fs::File;

        let mut file = File::create(path)?;
        write_header_to(&mut file, section::VERTEX_TIMESTAMPS).map_err(|e| {
            StorageError::io_error(format!("Failed to write timestamps header: {}", e))
        })?;

        let timestamps = self.timestamps.dump();
        let count = timestamps.len() as u32;
        file.write_all(&count.to_le_bytes())?;

        for ts in timestamps {
            file.write_all(&ts.to_le_bytes())?;
        }

        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_internal(path, true)
    }

    /// Load without applying deferred encodings (lazy load).
    /// Only use if you're certain encodings don't need to be applied immediately.
    pub fn load_lazy<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_internal(path, false)
    }

    fn load_internal<P: AsRef<Path>>(&mut self, path: P, eager_encode: bool) -> StorageResult<()> {
        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let meta_data = crate::storage::compression::read_decompressed(&meta_path)?;
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
        let label_name_len = u32::from_le_bytes(label_name_len_bytes) as usize;

        let mut label_name_bytes = vec![0u8; label_name_len];
        meta_cursor.read_exact(&mut label_name_bytes)?;
        self.label_name = String::from_utf8(label_name_bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

        let mut schema_len_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut schema_len_bytes)?;
        let schema_len = u32::from_le_bytes(schema_len_bytes) as usize;

        let mut schema_bytes = vec![0u8; schema_len];
        meta_cursor.read_exact(&mut schema_bytes)?;
        let schema_json = String::from_utf8(schema_bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        self.schema = serde_json::from_str(&schema_json)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

        // Rebuild property index cache
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }

        let id_indexer_path = path.join("id_indexer.bin");
        self.load_id_indexer(&id_indexer_path)?;

        let columns_path = path.join("columns.bin");
        self.load_columns(&columns_path)?;

        let timestamps_path = path.join("timestamps.bin");
        self.load_timestamps(&timestamps_path)?;

        // Apply deferred encodings if eager loading is requested
        if eager_encode {
            self.apply_deferred_encodings()?;
        }

        self.is_open = true;
        Ok(())
    }

    fn load_id_indexer(&mut self, path: &Path) -> StorageResult<()> {
        let data = crate::storage::compression::read_decompressed(path)?;
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
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = IdKey::from_bytes(&key_bytes)?;

            self.id_indexer.set_at(internal_id, key);
        }

        Ok(())
    }

    fn load_columns(&mut self, path: &Path) -> StorageResult<()> {
        let data = crate::storage::compression::read_decompressed(path)?;
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
        self.deferred_encodings.clear();

        for _ in 0..column_count {
            let mut name_len_bytes = [0u8; 4];
            cursor.read_exact(&mut name_len_bytes)?;
            let name_len = u32::from_le_bytes(name_len_bytes) as usize;

            let mut name_bytes = vec![0u8; name_len];
            cursor.read_exact(&mut name_bytes)?;
            let name = String::from_utf8(name_bytes)
                .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

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

            let mut encoding_byte_bytes = [0u8; 1];
            cursor.read_exact(&mut encoding_byte_bytes)?;
            let encoding_type = EncodingType::from_u8(encoding_byte_bytes[0]);

            // Store deferred encoding for later application
            // This allows lazy encoding on-demand or explicit application via ensure_encodings()
            if encoding_type != EncodingType::None {
                self.deferred_encodings.insert(name.clone(), encoding_type);
            }
        }

        Ok(())
    }

    fn load_timestamps(&mut self, path: &Path) -> StorageResult<()> {
        let data = crate::storage::compression::read_decompressed(path)?;
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

        let mut timestamps = Vec::with_capacity(count);
        for _ in 0..count {
            let mut ts_bytes = [0u8; 4];
            cursor.read_exact(&mut ts_bytes)?;
            timestamps.push(u32::from_le_bytes(ts_bytes));
        }

        self.timestamps.load(&timestamps);

        self.is_open = true;
        Ok(())
    }
}
