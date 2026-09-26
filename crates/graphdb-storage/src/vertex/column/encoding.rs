use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::{ColumnEncoding, EncodingType, FsstColumn, FsstEncoder};
use graphdb_core::NullBitmap;

use super::Column;

// ---------------------------------------------------------------------------
// Column encoding views
// ---------------------------------------------------------------------------

impl Column {
    /// Column-level view of the active encoding scheme: the first chunk (in
    /// row order) that carries one, including its pre-evict scheme when the
    /// chunk itself is evicted. `None` when no chunk is encoded.
    pub fn encoding_type(&self) -> EncodingType {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .map(|chunk| chunk.evicted_encoding())
            .find(|scheme| *scheme != EncodingType::None)
            .unwrap_or(EncodingType::None)
    }

    pub fn set_stats(&self, stats: ColumnStats) {
        *self.stats.write() = Some(stats);
        // Loaded data bypasses write_value, so the zone maps must be
        // rebuilt from the persisted column contents.
        self.rebuild_zone_maps();
    }

    /// Build one chunk-local encoding from the chunk's base values.
    ///
    /// The values are the overlay-merged, overflow-placeholder-substituted
    /// slice produced by the caller; nothing is read back from the column.
    pub(super) fn build_chunk_encoding(
        data_type: &DataType,
        values: &[Option<Value>],
        encoding_type: EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<ColumnEncoding> {
        match encoding_type {
            EncodingType::Fsst => build_fsst(data_type, values, fsst_max_symbols),
            EncodingType::Dictionary => build_dictionary(data_type, values),
            EncodingType::Rle => build_rle(data_type, values),
            EncodingType::BitPacking => build_bitpacked(data_type, values),
            EncodingType::Alp => build_alp(data_type, values),
            EncodingType::Constant => build_constant(values),
            EncodingType::None => Ok(ColumnEncoding::None),
        }
    }

    /// Compute statistics for the bytes that this column will persist.
    ///
    /// Encoded columns persist their per-chunk encoding metadata, while
    /// unencoded columns persist the raw buffers. Keeping the size
    /// calculation here makes flush-time statistics reflect the actual
    /// column format. Persistence itself serializes chunk vectors directly;
    /// this stats-only path may materialize flush buffers to size them.
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

        // Compressed footprint is the sum of the per-chunk encoding
        // metadata that the chunked record form persists.
        let mut encoded_metadata = 0u64;
        let mut any_encoded = false;
        for chunk in self.chunks.read().iter() {
            let state = chunk.read_state();
            if state.encoding.is_encoded() {
                any_encoded = true;
                let mut metadata = Vec::new();
                state.encoding.serialize_meta(&mut metadata)?;
                encoded_metadata = encoded_metadata.saturating_add(metadata.len() as u64);
            }
        }
        let compressed_size = if any_encoded {
            encoded_metadata
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
    pub fn rebuild_chunk_profiles(&self) {
        if self.maybe_rebuild_zone_maps_exact() {
            // Exact rebuild already refreshed zone maps and summaries from
            // current plus version chains; fall through to refresh chunk
            // encoding profiles below.
        } else {
            self.rebuild_zone_maps();
        }
        if self.chunks.read().is_empty() {
            return;
        }
        // Raw size is column-wide and identical for every chunk: resolve the
        // flush buffers once instead of materializing them per chunk.
        let (flush_data, flush_offsets, _) = self.get_flush_data();
        let raw_size = flush_data.len() as u64 + flush_offsets.len() as u64 * 8;
        let total_rows = self.len();
        // Windows are read once up front; the per-chunk refresh below only
        // touches segment latches, so no container lock is held across rows.
        let windows: Vec<(usize, usize, bool)> = {
            let chunks = self.chunks.read();
            chunks
                .iter()
                .map(|chunk| {
                    (
                        chunk.row_offset,
                        chunk.row_count,
                        chunk.read_state().residency.is_evicted(),
                    )
                })
                .collect()
        };
        if windows.is_empty() {
            return;
        }
        for (start, count_rows, evicted) in windows {
            // Evicted chunks keep their pre-evict profile: zone maps stay
            // resident in the segment state and keep serving.
            if evicted {
                continue;
            }
            let end = (start + count_rows).min(total_rows);
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
            // Refresh the owning chunk's profile by window match; a missing
            // window (concurrent-free exclusive load only) skips silently.
            let chunks = self.chunks.read();
            if let Some(chunk) = chunks
                .iter()
                .find(|chunk| chunk.row_offset == start && chunk.row_count == count_rows)
            {
                let enc_bytes = chunk.read_state().encoding.memory_usage() as u64;
                chunk.refresh_encoding_meta(count, all_null, min, max, enc_bytes, raw_size);
            }
        }
    }

    /// Persisted column statistics meta (min/max/null/distinct from the last
    /// flush), if any. Complements the always-fresh zone maps with counts.
    pub fn stats(&self) -> Option<crate::column_stats::ColumnStats> {
        self.stats.read().clone()
    }
}

// ---------------------------------------------------------------------------
// Encoding builders (chunk-local encodings over a caller-supplied value slice)
// ---------------------------------------------------------------------------

fn string_view(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) => Some(s.as_str()),
        Value::FixedString(s) => Some(s.as_str()),
        Value::Json(j) => Some(j.as_str()),
        _ => None,
    }
}

fn build_fsst(
    data_type: &DataType,
    values: &[Option<Value>],
    max_symbols: usize,
) -> StorageResult<ColumnEncoding> {
    if data_type != &DataType::String
        && data_type != &DataType::Json
        && !matches!(data_type, DataType::FixedString(_))
    {
        return Err(StorageError::not_supported(format!(
            "FSST encoding does not support type {:?}",
            data_type
        )));
    }

    let refs: Vec<Option<&str>> = values
        .iter()
        .map(|v| v.as_ref().and_then(string_view))
        .collect();
    let non_null: Vec<&str> = refs.iter().filter_map(|s| *s).collect();
    if non_null.is_empty() {
        return Ok(ColumnEncoding::None);
    }

    let encoder = FsstEncoder::train(&non_null, max_symbols);
    let mut encoded_data = Vec::with_capacity(values.len());
    let mut null_bitmap = NullBitmap::with_capacity(values.len());
    for s in &refs {
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

    Ok(ColumnEncoding::Fsst(FsstColumn {
        encoder,
        encoded_data,
        null_bitmap,
        updates_since_rebuild: 0,
    }))
}

fn build_dictionary(
    data_type: &DataType,
    values: &[Option<Value>],
) -> StorageResult<ColumnEncoding> {
    if data_type != &DataType::String && !matches!(data_type, DataType::FixedString(_)) {
        return Err(StorageError::not_supported(
            "Dictionary encoding only supports String and FixedString types".to_string(),
        ));
    }

    use crate::encoding::DictionaryColumn;

    let mut dict_col = DictionaryColumn::new();
    for (row, value) in values.iter().enumerate() {
        dict_col.set(row, value.as_ref())?;
    }

    Ok(ColumnEncoding::Dictionary(dict_col))
}

fn build_rle(data_type: &DataType, values: &[Option<Value>]) -> StorageResult<ColumnEncoding> {
    use crate::encoding::{RleBoolColumn, RleIntColumn};

    match data_type {
        DataType::Bool => {
            let mut rle_col = RleBoolColumn::new();
            for value in values {
                rle_col.append(value.as_ref())?;
            }
            Ok(ColumnEncoding::RleBool(rle_col))
        }
        DataType::SmallInt | DataType::Int | DataType::BigInt => {
            let mut rle_col = RleIntColumn::new();
            for value in values {
                rle_col.append(value.as_ref())?;
            }
            Ok(ColumnEncoding::RleInt(rle_col))
        }
        _ => Err(StorageError::not_supported(format!(
            "RLE encoding not supported for {:?}",
            data_type
        ))),
    }
}

fn build_bitpacked(
    data_type: &DataType,
    values: &[Option<Value>],
) -> StorageResult<ColumnEncoding> {
    use crate::encoding::BitPackedIntColumn;

    match data_type {
        DataType::SmallInt | DataType::Int | DataType::BigInt => {
            let owned: Vec<Option<Value>> = values.to_vec();
            let bp_col = BitPackedIntColumn::analyze(&owned, data_type.clone())?;
            Ok(ColumnEncoding::BitPacked(bp_col))
        }
        _ => Err(StorageError::not_supported(format!(
            "BitPacking encoding not supported for {:?}",
            data_type
        ))),
    }
}

fn build_constant(values: &[Option<Value>]) -> StorageResult<ColumnEncoding> {
    use crate::encoding::ConstantColumn;

    let owned: Vec<Option<Value>> = values.to_vec();
    if !ConstantColumn::should_use(&owned) {
        return Err(StorageError::invalid_operation(
            "Constant encoding requires all values to be identical".to_string(),
        ));
    }
    let first = owned.first().cloned().unwrap_or(None);
    Ok(ColumnEncoding::Constant(ConstantColumn::new(
        first,
        values.len(),
    )))
}

fn build_alp(data_type: &DataType, values: &[Option<Value>]) -> StorageResult<ColumnEncoding> {
    use crate::encoding::AlpColumn;

    match data_type {
        DataType::Float | DataType::Double => {
            let owned: Vec<Option<Value>> = values.to_vec();
            let alp_col = AlpColumn::analyze_values(&owned, data_type.clone())?;
            Ok(ColumnEncoding::Alp(alp_col))
        }
        _ => Err(StorageError::not_supported(format!(
            "ALP encoding not supported for {:?}",
            data_type
        ))),
    }
}
