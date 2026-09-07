use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::{ColumnEncoding, EncodingType, FsstColumn, FsstEncoder};
use graphdb_core::NullBitmap;

use super::Column;

// ---------------------------------------------------------------------------
// Column encoding methods
// ---------------------------------------------------------------------------

impl Column {
    pub fn encoding_type(&self) -> EncodingType {
        self.encoding.encoding_type()
    }

    pub fn encoding(&self) -> &ColumnEncoding {
        &self.encoding
    }

    pub fn set_stats(&mut self, stats: ColumnStats) {
        self.stats = Some(stats);
        // Loaded data bypasses write_value, so the zone maps must be
        // rebuilt from the persisted column contents.
        self.rebuild_zone_maps();
    }

    pub(super) fn sync_row_count_from_encoding(&mut self) {
        let encoded_len = self.encoding.len();
        self.inner_mut().resize(encoded_len);
    }

    pub fn apply_fsst_encoding(&mut self, max_symbols: usize) -> StorageResult<()> {
        if self.data_type != DataType::String && self.data_type != DataType::Json {
            return Err(StorageError::not_supported(format!(
                "FSST encoding does not support type {:?}",
                self.data_type
            )));
        }

        let mut strings: Vec<Option<String>> = Vec::with_capacity(self.len());
        for i in 0..self.len() {
            if self.is_null(i) {
                strings.push(None);
            } else {
                match self.get(i) {
                    Some(Value::String(s)) => strings.push(Some(s.to_string())),
                    Some(Value::Json(j)) => strings.push(Some(j.as_str().to_string())),
                    _ => strings.push(None),
                }
            }
        }

        let string_refs: Vec<Option<&str>> = strings.iter().map(|s| s.as_deref()).collect();
        let non_null: Vec<&str> = string_refs.iter().filter_map(|s| *s).collect();

        if non_null.is_empty() {
            return Ok(());
        }

        let encoder = FsstEncoder::train(&non_null, max_symbols);

        let mut encoded_data = Vec::with_capacity(self.len());
        let mut null_bitmap = NullBitmap::with_capacity(self.len());

        for s in &string_refs {
            match s {
                Some(val) => {
                    encoded_data.push(encoder.encode(val));
                    null_bitmap.push(false);
                }
                None => {
                    encoded_data.push(Vec::new());
                    null_bitmap.push(true);
                }
            }
        }

        let fsst_col = FsstColumn {
            encoder,
            encoded_data,
            null_bitmap,
            updates_since_rebuild: 0,
        };

        self.encoding = ColumnEncoding::Fsst(fsst_col);

        Ok(())
    }

    pub fn apply_dictionary_encoding(&mut self) -> StorageResult<()> {
        if self.data_type != DataType::String {
            return Err(StorageError::not_supported(
                "Dictionary encoding only supports String type".to_string(),
            ));
        }

        use crate::encoding::DictionaryColumn;

        let mut dict_col = DictionaryColumn::new();
        for i in 0..self.len() {
            let value = self.get(i);
            dict_col.set(i, value.as_ref())?;
        }

        self.encoding = ColumnEncoding::Dictionary(dict_col);

        Ok(())
    }

    pub fn apply_rle_encoding(&mut self) -> StorageResult<()> {
        use crate::encoding::{RleBoolColumn, RleIntColumn};

        match self.data_type {
            DataType::Bool => {
                let mut rle_col = RleBoolColumn::new();
                for i in 0..self.len() {
                    let value = self.get(i);
                    rle_col.append(value.as_ref())?;
                }
                self.encoding = ColumnEncoding::RleBool(rle_col);
            }
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                let mut rle_col = RleIntColumn::new();
                for i in 0..self.len() {
                    let value = self.get(i);
                    rle_col.append(value.as_ref())?;
                }
                self.encoding = ColumnEncoding::RleInt(rle_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "RLE encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }

    pub fn apply_bitpacking_encoding(&mut self) -> StorageResult<()> {
        use crate::encoding::BitPackedIntColumn;

        match self.data_type {
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                let mut values: Vec<Option<Value>> = Vec::with_capacity(self.len());
                for i in 0..self.len() {
                    values.push(self.get(i));
                }
                let bp_col = BitPackedIntColumn::analyze(&values, self.data_type.clone())?;
                self.encoding = ColumnEncoding::BitPacked(bp_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "BitPacking encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }

    pub fn apply_constant_encoding(&mut self) -> StorageResult<()> {
        use crate::encoding::ConstantColumn;

        let mut values: Vec<Option<Value>> = Vec::with_capacity(self.len());
        for i in 0..self.len() {
            values.push(self.get(i));
        }
        if !ConstantColumn::should_use(&values) {
            return Err(StorageError::invalid_operation(
                "Constant encoding requires all values to be identical".to_string(),
            ));
        }
        let first = values.first().cloned().unwrap_or(None);
        let col = ConstantColumn::new(first, self.len());
        self.encoding = ColumnEncoding::Constant(col);
        Ok(())
    }

    pub fn apply_alp_encoding(&mut self) -> StorageResult<()> {
        use crate::encoding::AlpColumn;

        match self.data_type {
            DataType::Float | DataType::Double => {
                let mut values: Vec<Option<Value>> = Vec::with_capacity(self.len());
                for i in 0..self.len() {
                    values.push(self.get(i));
                }
                let alp_col = AlpColumn::analyze_values(&values, self.data_type.clone())?;
                self.encoding = ColumnEncoding::Alp(alp_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "ALP encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }

    /// Compute statistics for the bytes that this column will persist.
    ///
    /// Encoded columns persist their encoding metadata, while unencoded
    /// columns persist the raw buffers. Keeping the size calculation here
    /// makes flush-time statistics reflect the actual column format.
    ///
    /// Aggregation is streaming: rows are visited once without materializing
    /// a value vector or a distinct hash set (HLL-backed estimate).
    pub fn compute_stats(&self) -> StorageResult<ColumnStats> {
        self.compute_stats_streaming()
    }

    /// Streaming stats aggregation over chunks without row materialization.
    pub fn compute_stats_streaming(&self) -> StorageResult<ColumnStats> {
        let (data, offsets, bitmap) = self.get_flush_data();
        let raw_size = data
            .len()
            .saturating_add(offsets.len().saturating_mul(std::mem::size_of::<u64>()))
            .saturating_add(
                bitmap
                    .as_ref()
                    .map(|bits| bits.as_raw_slice().len())
                    .unwrap_or(0),
            ) as u64;

        let compressed_size = if self.encoding.is_encoded() {
            let mut metadata = Vec::new();
            self.encoding.serialize_meta(&mut metadata)?;
            metadata.len() as u64
        } else {
            raw_size
        };

        let encoding_type = self.encoding_type();
        let iter = (0..self.len()).map(|row_idx| self.get(row_idx));
        Ok(crate::column_stats::compute_stats_streaming(
            iter,
            encoding_type,
            compressed_size,
            raw_size,
        ))
    }

    /// Rebuild zone maps and per-chunk encoding profiles in one pass.
    pub fn rebuild_chunk_profiles(&mut self) {
        self.rebuild_zone_maps();
        if self.chunks.is_empty() {
            return;
        }
        // Raw size is column-wide and identical for every chunk: resolve the
        // flush buffers once instead of materializing them per chunk.
        let (flush_data, flush_offsets, _) = self.get_flush_data();
        let raw_size = flush_data.len() as u64 + flush_offsets.len() as u64 * 8;
        let total_rows = self.len();
        for idx in 0..self.chunks.len() {
            let start = self.chunks[idx].row_offset;
            let end = (start + self.chunks[idx].row_count).min(total_rows);
            let mut min: Option<Value> = None;
            let mut max: Option<Value> = None;
            let mut count = 0u32;
            for row in start..end {
                if let Some(v) = self.get(row) {
                    count += 1;
                    if min.as_ref().is_none_or(|m| &v < m) {
                        min = Some(v.clone());
                    }
                    if max.as_ref().is_none_or(|m| &v > m) {
                        max = Some(v.clone());
                    }
                }
            }
            let all_null = count == 0;
            let enc_bytes = self.chunks[idx].encoding.memory_usage() as u64;
            self.chunks[idx].refresh_encoding_meta(count, all_null, min, max, enc_bytes, raw_size);
        }
    }

    /// Persisted column statistics meta (min/max/null/distinct from the last
    /// flush), if any. Complements the always-fresh zone maps with counts.
    pub fn stats(&self) -> Option<&crate::column_stats::ColumnStats> {
        self.stats.as_ref()
    }
}
