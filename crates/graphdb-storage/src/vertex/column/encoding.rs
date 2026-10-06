use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::AtomicU64;

use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::{ColumnEncoding, EncodingType, FsstColumn, FsstEncoder};
use graphdb_core::NullBitmap;
use parking_lot::RwLock;

use super::chunk::ColumnChunk;
use super::chunk_residency::{next_tick, ChunkResidency};
use super::Column;

// ---------------------------------------------------------------------------
// Column encoding views
// ---------------------------------------------------------------------------

impl Column {
    /// Pick one encoding for the whole column by profiling each chunk
    /// independently and voting for the most common non-`None` chunk choice.
    ///
    /// Streaming: each chunk is summarized into a [`ChunkProfile`] as it is
    /// walked, so no column-wide value vector is ever built. Hot chunks vote
    /// `None` through the profile, and a multi-chunk column needs a majority
    /// so one odd chunk cannot force a column-wide encoding.
    pub fn select_encoding(&self, selector: &crate::encoding::EncodingSelector) -> EncodingType {
        use crate::encoding::profile_chunk;
        use std::collections::HashMap;

        if self.is_empty() {
            return EncodingType::None;
        }
        let capacity = self.chunk_capacity().max(1);
        let total = self.len();
        let n_chunks = total.div_ceil(capacity).max(1);
        let mut votes: HashMap<u8, usize> = HashMap::new();
        for ci in 0..n_chunks {
            let start = ci * capacity;
            let end = (start + capacity).min(total);
            let hot = self.chunk_needs_recode_for_row(start);
            let profile = profile_chunk((start..end).map(|r| self.get(r)), &self.data_type, hot);
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
        if n_chunks > 1 && best_count * 2 <= n_chunks {
            return EncodingType::None;
        }
        EncodingType::from_u8(best_tag)
    }

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
            .saturating_add(offsets.len().saturating_mul(std::mem::size_of::<u32>()))
            .saturating_add(
                bitmap
                    .as_ref()
                    .map(|bits| bits.as_raw_slice().len())
                    .unwrap_or(0),
            ) as u64;

        // Compressed footprint is the resident encoding memory across
        // chunks, covering dictionaries, codebooks and encoded payloads.
        let mut encoded_bytes = 0u64;
        let mut any_encoded = false;
        for chunk in self.chunks.read().iter() {
            let state = chunk.read_state();
            if state.encoding.is_encoded() {
                any_encoded = true;
                encoded_bytes = encoded_bytes.saturating_add(state.encoding.memory_usage() as u64);
            }
        }
        let compressed_size = if any_encoded { encoded_bytes } else { raw_size };

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
        let raw_size = flush_data.len() as u64 + flush_offsets.len() as u64 * 4;
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

// ---------------------------------------------------------------------------
// Chunk-level encoding application and flush views
// ---------------------------------------------------------------------------

impl Column {
    /// Encode one row slice into a chunk-local encoding of the given type.
    /// A failed or infeasible build yields `None` (the chunk stays raw).
    pub(super) fn encode_slice(
        values: &[Option<Value>],
        data_type: &DataType,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> ColumnEncoding {
        if values.is_empty() {
            return ColumnEncoding::None;
        }
        Self::build_chunk_encoding(data_type, values, encoding_type, fsst_max_symbols)
            .unwrap_or(ColumnEncoding::None)
    }

    /// Enforce the chunk window invariant: windows are contiguous from row
    /// zero and no chunk exceeds `chunk_capacity`.
    ///
    /// Writes and loads maintain this shape directly, so this is normally a
    /// no-op. It does real work only after a capacity shrink strands an
    /// oversized chunk (split at the capacity boundary) and it drops
    /// reserved zero-row tail chunks left behind by `reserve`.
    ///
    /// Exclusive-only: rewriting windows while point operations route by
    /// them would misroute reads and writes. All callers run under the
    /// shard write lock.
    pub fn materialize_chunks(&self) {
        let mut chunks = self.chunks.write();
        while chunks.last().is_some_and(|chunk| chunk.row_count == 0) {
            chunks.pop();
        }
        let capacity = self.chunk_capacity().max(1);
        let mut idx = 0;
        while idx < chunks.len() {
            if chunks[idx].row_count <= capacity {
                idx += 1;
                continue;
            }
            let offset = chunks[idx].row_offset;
            let at = capacity.min(chunks[idx].row_count);
            let right_count = chunks[idx].row_count - at;
            // Splits operate on raw buffers: decode first so the cut never
            // drops overlay entries (decode merges them into the base).
            self.decode_locked(&chunks, idx);
            let right_state = {
                let chunk = &chunks[idx];
                let mut state = chunk.write_state();
                debug_assert_eq!(state.overlay.len(), 0);
                let (left_raw, right_raw) = Self::split_raw(&state.raw, at);
                state.raw = left_raw;
                let right_chains = state.version_chains.as_mut().map(|c| {
                    let mut right = std::collections::HashMap::new();
                    c.retain(|local, chain| {
                        if *local < at {
                            true
                        } else {
                            right.insert(*local - at, std::mem::take(chain));
                            false
                        }
                    });
                    right
                });
                let right_vis = state.visibility.split_off(at);
                // Dirty pages are global ids: a page stays with the side
                // holding its first row. A straddling page stays left; its
                // flush covers both sides at page granularity.
                let cut_row = offset + at;
                let mut right_dirty = BTreeSet::new();
                state.dirty_pages.retain(|page| {
                    if (*page as usize)
                        .saturating_mul(crate::persistence::dirty_page::ROWS_PER_PAGE)
                        < cut_row
                    {
                        true
                    } else {
                        right_dirty.insert(*page);
                        false
                    }
                });
                let mut right_overflow = HashMap::new();
                state.overflow_rows.retain(|local, handle| {
                    if (*local as usize) < at {
                        true
                    } else {
                        right_overflow.insert(*local - at as u32, *handle);
                        false
                    }
                });
                crate::vertex::column::chunk::ChunkState {
                    raw: right_raw,
                    encoding: ColumnEncoding::None,
                    overlay: super::chunk_encoding::UpdateOverlay::new(
                        super::chunk_encoding::overlay_capacity_for(right_count),
                    ),
                    encoding_meta: crate::encoding::ChunkEncodingMeta::default(),
                    updates_since_encode: 0,
                    version_chains: right_chains,
                    visibility: right_vis,
                    dirty_pages: right_dirty,
                    overflow_rows: right_overflow,
                    residency: ChunkResidency::Resident,
                }
            };
            chunks[idx].row_count = at;
            let right = ColumnChunk {
                row_offset: offset + at,
                row_count: right_count,
                element_size: chunks[idx].element_size,
                state: RwLock::new(right_state),
                last_access: AtomicU64::new(next_tick()),
            };
            chunks.insert(idx + 1, right);
            idx += 1;
        }
    }

    /// Per-chunk flush view: window plus cloned payload descriptors for
    /// persistence serialization. The clones are taken under one segment
    /// read latch each so the serialized record is chunk-consistent.
    /// Evicted chunks (possible only when flush-time promotion failed) are
    /// materialized row-wise from their snapshot into raw buffers.
    pub(crate) fn chunk_flush_view(&self, idx: usize) -> Option<super::ChunkFlushView> {
        let chunks = self.chunks.read();
        let chunk = chunks.get(idx)?;
        let state = chunk.read_state();
        let raw_form = !state.encoding.is_encoded();
        let (raw_data, raw_offsets, raw_bitmap) = if !raw_form {
            (Vec::new(), Vec::new(), None)
        } else if state.residency.is_evicted() {
            let (data, offsets, bitmap) = self.values_into_buffers(
                (chunk.row_offset..chunk.row_offset + chunk.row_count)
                    .map(|row| self.raw_base_value_in(&chunks, row)),
            );
            (data, offsets, bitmap)
        } else {
            state.raw.as_storage().get_flush_data()
        };
        let overlay: Vec<(u32, Option<Value>)> = if state.residency.is_evicted() {
            // Evicted windows carry no live overlay (writes promote first),
            // and the materialized buffers above already merged any values.
            Vec::new()
        } else {
            state.overlay.iter().map(|(k, v)| (*k, v.clone())).collect()
        };
        Some(super::ChunkFlushView {
            row_offset: chunk.row_offset,
            row_count: chunk.row_count,
            encoding_meta: state.encoding_meta.clone(),
            encoding: state.encoding.clone(),
            overlay,
            raw_form,
            raw_data,
            raw_offsets,
            raw_bitmap,
        })
    }

    /// Every chunk's flush view, in row order.
    pub(crate) fn chunk_flush_views(&self) -> Vec<super::ChunkFlushView> {
        let count = self.chunks.read().len();
        (0..count)
            .filter_map(|idx| self.chunk_flush_view(idx))
            .collect()
    }

    /// Per-chunk encoding metadata: (chunk_idx, encoding type, row count).
    /// Evicted chunks report their pre-evict scheme so sidecars and chunk
    /// profiles keep describing the flushed layout.
    pub fn chunk_encoding_metadata(&self) -> Vec<(usize, crate::encoding::EncodingType, usize)> {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.evicted_encoding(), c.row_count))
            .collect()
    }

    /// Re-evict one resident chunk from checkpoint sidecar pages without
    /// decoding them onto the heap. Matches by row window; a mismatch or
    /// an already-evicted chunk keeps current state and reports false so
    /// the caller stays resident. Used by reload to restore the persisted
    /// eviction state. Exclusive-only (load path).
    pub fn restore_mapped_chunk(
        &self,
        record: super::chunk_residency::MappedChunk,
        map: &std::sync::Arc<memmap2::Mmap>,
    ) -> bool {
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.iter().find(|c| {
            c.row_offset == record.row_offset as usize && c.row_count == record.rows as usize
        }) else {
            log::warn!(
                "snapshot sidecar window [{}, {}) matches no chunk of column {}",
                record.row_offset,
                record.row_offset as usize + record.rows as usize,
                self.name,
            );
            return false;
        };
        let mut state = chunk.write_state();
        if !state.residency.is_resident() {
            return false;
        }
        let snapshot = super::chunk_residency::EvictedSnapshot::from_mapped(
            record.rows,
            record.encoding,
            record.meta,
            record.uncompressed_bytes,
            map.clone(),
            record.frames,
        );
        state.raw.as_storage_mut().clear();
        state.encoding = crate::encoding::ColumnEncoding::None;
        state.residency = ChunkResidency::Evicted(snapshot);
        true
    }

    /// Apply one selected encoding to this column, chunk by chunk.
    ///
    /// Each resident chunk selects and stores its own encoding so point
    /// updates only decode the affected chunk. Exclusive-only (flush/encode
    /// path): it rewrites chunk payloads wholesale.
    pub fn apply_selected_encoding(
        &self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        if self.is_empty() {
            return Ok(());
        }
        self.apply_encoding_to_chunks(encoding_type, fsst_max_symbols)
    }

    /// Apply an encoding type independently per chunk.
    ///
    /// Each chunk is resolved and encoded in isolation (bounded by chunk
    /// capacity): no full-column value vector is ever materialized. Hot
    /// chunks encode first so write hotspots become evictable sooner; cold
    /// chunks keep their rhythm. When a chunk selects `None`, its
    /// overlay-merged values are written back to the raw buffer before the
    /// overlay is cleared so no update is lost. Overlay-full chunks already
    /// mark hot on the write path and never wait for a whole-column
    /// fallback.
    pub fn apply_encoding_to_chunks(
        &self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        // Bind the check so the read guard drops before materialize_chunks:
        // parking_lot locks are not reentrant.
        let empty = self.chunks.read().is_empty();
        if empty {
            self.materialize_chunks();
        }
        if self.chunks.read().is_empty() {
            return Ok(());
        }
        // Encoding rebuilds need decoded buffers: promote evicted chunks
        // first so every chunk below is resident. Load failures propagate
        // instead of silently encoding a partial column.
        let len = self.chunks.read().len();
        for idx in 0..len {
            self.ensure_resident(idx)?;
        }
        let total = self.len();
        let order: Vec<usize> = {
            let chunks = self.chunks.read();
            let mut scored: Vec<(u64, usize)> = chunks
                .iter()
                .enumerate()
                .map(|(idx, chunk)| {
                    let hot = chunk.read_state().updates_since_encode;
                    (hot, idx)
                })
                .collect();
            scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
            scored.into_iter().map(|(_, idx)| idx).collect()
        };
        for &idx in &order {
            let (start, end) = {
                let chunks = self.chunks.read();
                let Some(chunk) = chunks.get(idx) else {
                    continue;
                };
                let start = chunk.row_offset;
                (start, start.saturating_add(chunk.row_count).min(total))
            };
            // Per-chunk slice only (overlay-merged base; overflow rows as
            // placeholders whose payloads stay in the sidecar).
            let values: Vec<Option<Value>> =
                (start..end).map(|r| self.encoding_base_value(r)).collect();
            let selected = match encoding_type {
                crate::encoding::EncodingType::None => crate::encoding::EncodingType::None,
                _ => {
                    let selector = crate::encoding::EncodingSelector::default();
                    selector.select_for_chunk(&self.data_type, &values)
                }
            };
            let chunks = self.chunks.read();
            let Some(chunk) = chunks.get(idx) else {
                continue;
            };
            if selected == crate::encoding::EncodingType::None {
                // Preserve overlay-merged values: the raw buffer still holds
                // the pre-overlay base, so write merged values back first.
                drop(chunks);
                for (off, v) in values.iter().enumerate() {
                    let row = start + off;
                    let _ = self.write_raw_inner(row, v.as_ref());
                }
                let chunks = self.chunks.read();
                if let Some(chunk) = chunks.get(idx) {
                    let mut state = chunk.write_state();
                    state.encoding = ColumnEncoding::None;
                    state.overlay.clear();
                    state.updates_since_encode = 0;
                    state.encoding_meta.scheme = crate::encoding::EncodingType::None;
                    state.encoding_meta.num_values =
                        values.iter().filter(|v| v.is_some()).count() as u32;
                    state.encoding_meta.all_null = state.encoding_meta.num_values == 0;
                }
                continue;
            }
            // Encode this chunk slice in isolation (chunk-local indexes).
            let encoded = Self::encode_slice(&values, &self.data_type, selected, fsst_max_symbols);
            if encoded.is_encoded() {
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                let mut state = chunk.write_state();
                state.encoding = encoded;
                state.overlay.clear();
                state.updates_since_encode = 0;
                state.encoding_meta.scheme = state.encoding.encoding_type();
                state.encoding_meta.num_values = num_values;
                state.encoding_meta.all_null = num_values == 0;
                state.encoding_meta.compressed_size = state.encoding.memory_usage() as u64;
            }
        }
        Ok(())
    }
}
