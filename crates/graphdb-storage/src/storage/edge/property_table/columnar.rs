//! Columnar integration: ColumnStore sync, projected reads, fast paths.

use super::*;

impl PropertyTable {
    /// Fast path: update a single property value via direct byte manipulation.
    /// Only applicable for fixed-size schemas where byte offsets are known.
    /// Skips full deserialize → merge → serialize cycle.
    pub(super) fn set_property_fixed_size(
        &mut self,
        row_idx: usize,
        offset: u32,
        col_idx: usize,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        // Storage-layer write-write conflict detection (direct callers such as
        // `set_property_by_id` bypass `set_property`'s check).
        self.check_write_conflict(row_idx, offset, ts)?;

        let Some(record) = self.records[row_idx].as_ref() else {
            return Err(StorageError::invalid_offset(offset));
        };

        // Clone the old data and overwrite the target property's bytes
        let mut new_data = record.data.clone();
        self.serialize_value_at_offset(&mut new_data, value.as_ref(), col_idx)?;

        // MVCC: supersede the current version, keeping the prior row as a
        // before-image for snapshot reads.
        self.supersede_current(row_idx, offset, ts);

        // Replace with new record (same position, new data + timestamp)
        let new_record_obj = PropertyRecord::new(new_data, ts);
        self.used_data_bytes += new_record_obj.data.len();
        self.records[row_idx] = Some(new_record_obj);

        // Columnar sync (for direct callers like set_property_by_id).
        if let Some(schema) = self.schema.get(col_idx) {
            let col_name = schema.name.clone();
            let _ =
                self.column_store
                    .set_property_versioned(row_idx, &col_name, value.as_ref(), ts);
            self.refresh_zone_map_for_row(row_idx);
        }

        Ok(())
    }

    pub fn column_values(&self, col_idx: usize) -> Vec<Option<Value>> {
        if col_idx >= self.schema.len() {
            return Vec::new();
        }
        let col_name = self.schema[col_idx].name.clone();
        // Prefer columnar store (zero-copy, OLAP path) when available.
        if let Some(col) = self.column_store.get_column(&col_name) {
            let mut values = Vec::with_capacity(self.records.len());
            for row_idx in 0..self.records.len() {
                // Use current view (None = latest) for stats; respects live rows.
                if self.records[row_idx].is_none()
                    || self.records[row_idx]
                        .as_ref()
                        .is_some_and(|r| r.delete_ts.is_some())
                {
                    values.push(None);
                } else {
                    values.push(col.get(row_idx));
                }
            }
            if !values.is_empty() {
                return values;
            }
        }
        // The columnar store is rebuilt on load and dual-written on write,
        // so a missing column here means an internal invariant violation.
        debug_assert!(
            false,
            "column_values: columnar store missing column '{col_name}'"
        );
        Vec::new()
    }

    /// Column pruning: read only `projection` columns for one row at `query_ts`.
    /// Returns `None` if the row does not exist or is not visible at `query_ts`.
    pub fn get_projected(
        &self,
        offset: u32,
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.records.len() {
            return None;
        }
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        // Check row visibility (current record vs chain).
        let visible = match query_ts {
            None => self.records[row_idx]
                .as_ref()
                .is_some_and(|r| r.delete_ts.is_none()),
            Some(t) => {
                if let Some(rec) = self.records[row_idx].as_ref() {
                    if rec.is_visible_at(t) {
                        true
                    } else {
                        self.chain_records
                            .get(row_idx)
                            .is_some_and(|chain| chain.iter().any(|r| r.is_visible_at(t)))
                    }
                } else {
                    false
                }
            }
        };
        if !visible {
            return None;
        }
        if projection.is_empty() {
            return self.get(offset, query_ts);
        }
        // Columnar path: read only requested columns via ColumnStore (MVCC-aware).
        let mut out = Vec::with_capacity(projection.len());
        for col_name in projection {
            if let Some(col) = self.column_store.get_column(col_name) {
                let val = if query_ts.is_some() {
                    col.get_at_ts(row_idx, ts)
                } else {
                    col.get(row_idx)
                };
                out.push((col_name.clone(), val));
            } else {
                // Column not in column store (legacy); fallback to row decode.
                if let Some(row) = self.get(offset, query_ts) {
                    if let Some((_, v)) = row.into_iter().find(|(n, _)| n == col_name) {
                        out.push((col_name.clone(), v));
                    } else {
                        out.push((col_name.clone(), None));
                    }
                } else {
                    out.push((col_name.clone(), None));
                }
            }
        }
        Some(out)
    }

    /// Batch column pruning: read `projection` columns for many offsets.
    /// Output order matches input order; missing rows yield `None`.
    pub fn get_projected_batch(
        &self,
        offsets: &[u32],
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Vec<ProjectedRow> {
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        // Group by chunk for zone-map pruning opportunity.
        let mut out = Vec::with_capacity(offsets.len());
        for &off in offsets {
            out.push(self.get_projected(off, projection, query_ts));
        }
        // Prefetch hint for columnar path.
        let row_indices: Vec<usize> = offsets
            .iter()
            .filter_map(|o| prop_offset_to_index(*o))
            .collect();
        if !projection.is_empty() && !row_indices.is_empty() {
            // Warm zone-map access for batch.
            let _ = self
                .column_store
                .get_projected_batch_at_ts(&row_indices, projection, ts);
        }
        out
    }

    /// Apply per-column compression encoding (ALP / bitpacking / dict / etc.)
    /// Delegates to `ColumnStore`. OLAP scans benefit from reduced IO.
    pub fn apply_column_encoding(
        &mut self,
        col_name: &str,
        encoding: EncodingType,
    ) -> StorageResult<()> {
        self.column_store
            .apply_encoding_to_column(col_name, encoding, 4096)
    }

    /// Prefetch a single property offset into CPU cache
    /// This is a no-op on most systems but signals intent for cache optimization
    #[inline]
    pub fn prefetch(&self, offset: u32) {
        if let Some(row_idx) = prop_offset_to_index(offset) {
            if row_idx < self.records.len() {
                if let Some(record) = &self.records[row_idx] {
                    // Prefetch the data location to L1/L2 cache
                    #[allow(unsafe_code)]
                    unsafe {
                        let addr = record.data.as_ptr();
                        // Use a volatile read to ensure prefetch happens
                        std::ptr::read_volatile(addr);
                    }
                }
            }
        }
    }

    /// Prefetch multiple property offsets in batch
    /// Improves cache locality for bulk operations
    pub fn prefetch_batch(&self, offsets: &[u32]) {
        for offset in offsets {
            self.prefetch(*offset);
        }
    }

    /// Fast path deserialization for fixed-size schemas
    /// Skips null checks and type dispatching for 2-3x speedup
    pub fn get_fast(
        &self,
        offset: u32,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        if !self.is_schema_fixed_size() {
            return self.get(offset, query_ts);
        }

        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.records.len() {
            return None;
        }

        let record = self.resolve_version(row_idx, query_ts)?;

        let record_data = &record.data;

        // Fast path: directly deserialize without null checks
        let mut cursor = Cursor::new(record_data);
        let mut result = Vec::with_capacity(self.schema.len());

        for schema in &self.schema {
            // The row format still contains a per-column null marker; it must be
            // consumed before the value bytes.
            let mut null_marker = [0u8; 1];
            if cursor.read_exact(&mut null_marker).is_err() {
                return None;
            }
            if null_marker[0] == 0 {
                result.push((schema.name.clone(), None));
                continue;
            }
            match &schema.data_type {
                DataType::Bool => {
                    let mut b = [0u8; 1];
                    if cursor.read_exact(&mut b).is_err() {
                        return None;
                    }
                    result.push((schema.name.clone(), Some(Value::Bool(b[0] != 0))));
                }
                DataType::SmallInt => {
                    let mut buf = [0u8; 2];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::SmallInt(i16::from_le_bytes(buf))),
                    ));
                }
                DataType::Int => {
                    let mut buf = [0u8; 4];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Int(i32::from_le_bytes(buf))),
                    ));
                }
                DataType::BigInt => {
                    let mut buf = [0u8; 8];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::BigInt(i64::from_le_bytes(buf))),
                    ));
                }
                DataType::Float => {
                    let mut buf = [0u8; 4];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Float(f32::from_le_bytes(buf))),
                    ));
                }
                DataType::Double => {
                    let mut buf = [0u8; 8];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Double(f64::from_le_bytes(buf))),
                    ));
                }
                _ => {
                    // Should not reach here due to is_schema_fixed_size check
                    return None;
                }
            }
        }

        Some(result)
    }

    /// Batch retrieval of properties, sorted by offset for cache locality
    /// Returns results in original order via the provided iterator
    #[allow(clippy::type_complexity)]
    pub fn get_batch<'a, I>(
        &'a self,
        offsets: I,
        query_ts: Option<Timestamp>,
    ) -> Vec<Option<Vec<(String, Option<Value>)>>>
    where
        I: IntoIterator<Item = &'a u32>,
    {
        let offsets: Vec<_> = offsets.into_iter().collect();
        let mut indexed: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(idx, offset)| (idx, **offset))
            .collect();

        // Sort by offset to improve cache locality
        indexed.sort_by_key(|(_, offset)| *offset);

        // Prefetch all offsets
        for (_, offset) in &indexed {
            self.prefetch(*offset);
        }

        // Retrieve in sorted order
        let sorted_results: Vec<_> = indexed
            .iter()
            .map(|(_, offset)| {
                self.get_fast(*offset, query_ts)
                    .or_else(|| self.get(*offset, query_ts))
            })
            .collect();

        // Restore original order
        let mut results = vec![None; offsets.len()];
        for (orig_idx, sorted_result) in indexed.iter().zip(sorted_results) {
            results[orig_idx.0] = sorted_result;
        }

        results
    }
}
