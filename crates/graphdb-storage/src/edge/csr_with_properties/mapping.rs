use super::{CsrWithProperties, RowVisibility, UNMAPPED_ROW};
use graphdb_core::types::{EdgeId, Timestamp, INVALID_EDGE_ID};
use graphdb_core::{StorageError, StorageResult, Value};

impl CsrWithProperties {
    fn segment_of(slot: usize) -> (usize, usize) {
        debug_assert_eq!(Self::EDGE_MAP_SEGMENT_ROWS, 1024);
        (
            slot / Self::EDGE_MAP_SEGMENT_ROWS,
            slot % Self::EDGE_MAP_SEGMENT_ROWS,
        )
    }

    /// Row mapped to `edge_id`, or `None` for unmapped ids.
    ///
    /// Edge ids are table-allocated dense values; untouched segments hold no
    /// allocation, so this is one segment lookup plus one indexed read.
    pub(crate) fn mapped_row(&self, edge_id: EdgeId) -> Option<usize> {
        let (seg, off) = Self::segment_of(edge_id.0 as usize);
        let pos = self.edge_map_segments.get(seg)?.as_ref()?[off];
        (pos != UNMAPPED_ROW).then_some(pos as usize)
    }

    /// Record the `edge_id` to `row_idx` mapping, allocating the segment on demand.
    ///
    /// Rejects the unassignable gap sentinel explicitly: gap slots never
    /// own property rows.
    pub(crate) fn map_insert(&mut self, edge_id: EdgeId, row_idx: usize) -> StorageResult<()> {
        if edge_id == INVALID_EDGE_ID {
            return Err(StorageError::invalid_operation(
                "unassignable edge id owns no property row",
            ));
        }
        let slot = edge_id.0 as usize;
        let (seg, off) = Self::segment_of(slot);
        if seg >= self.edge_map_segments.len() {
            self.edge_map_segments.resize_with(seg + 1, || None);
        }
        let segment = self.edge_map_segments[seg]
            .get_or_insert_with(|| Box::new([UNMAPPED_ROW; Self::EDGE_MAP_SEGMENT_ROWS]));
        if segment[off] == UNMAPPED_ROW {
            self.edge_map_len += 1;
        }
        segment[off] = row_idx as u32;
        Ok(())
    }

    /// Drop the mapping for `edge_id`, returning its former row.
    pub(crate) fn map_remove(&mut self, edge_id: EdgeId) -> Option<usize> {
        let slot = edge_id.0 as usize;
        let (seg, off) = Self::segment_of(slot);
        let segment = self.edge_map_segments.get_mut(seg)?.as_mut()?;
        if segment[off] == UNMAPPED_ROW {
            return None;
        }
        let former = segment[off] as usize;
        segment[off] = UNMAPPED_ROW;
        self.edge_map_len = self.edge_map_len.saturating_sub(1);
        if segment.iter().all(|pos| *pos == UNMAPPED_ROW) {
            self.edge_map_segments[seg] = None;
        }
        self.truncate_empty_tail_segments();
        Some(former)
    }

    /// Release trailing empty segments so churned id ranges never pin memory.
    fn truncate_empty_tail_segments(&mut self) {
        while self
            .edge_map_segments
            .last()
            .is_some_and(|seg| seg.is_none())
        {
            self.edge_map_segments.pop();
        }
    }

    pub(crate) fn ensure_row_aux_len(&mut self, len: usize) {
        if self.row_to_edge.len() < len {
            self.row_to_edge.resize(len, None);
        }
    }

    /// Allocate a new row and populate it with the given values.
    /// Returns the row index (0-based).
    pub(crate) fn allocate_row(
        &mut self,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<usize> {
        // Resolve names through the schema index once per call; the hot
        // write path passes pre-resolved positions straight to
        // `allocate_row_at` instead. Unknown names keep the historical
        // behavior of falling back to the column default.
        let mut positioned: Vec<(usize, Value)> = Vec::with_capacity(values.len());
        for (name, value) in values {
            if let Some(&idx) = self.column_index.get(name.as_str()) {
                positioned.push((idx, value.clone()));
            }
        }
        self.allocate_row_at(&positioned, create_ts)
    }

    /// Allocate a new row from pre-resolved `(column position, value)` pairs.
    ///
    /// Positions come from the schema index held by the caller, so this
    /// performs no name lookup or string comparison per edge. Columns with
    /// no provided value take their default. An out-of-range position is a
    /// caller bug and fails loudly instead of writing the wrong column.
    pub(crate) fn allocate_row_at(
        &mut self,
        positioned: &[(usize, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<usize> {
        for (idx, _) in positioned {
            if *idx >= self.property_schema.len() {
                return Err(StorageError::column_not_found(format!(
                    "property column position out of range: {}",
                    idx
                )));
            }
        }
        let row_idx = if let Some(free_off) = self.free_list.pop() {
            let idx = free_off as usize;
            if idx >= self.visibility.len() {
                self.visibility.resize(idx + 1, RowVisibility::new(0));
            }
            self.visibility[idx] = RowVisibility::new(create_ts);
            self.row_count += 1;
            idx
        } else {
            let idx = self.visibility.len();
            self.visibility.push(RowVisibility::new(create_ts));
            self.row_count += 1;
            idx
        };
        self.ensure_row_aux_len(self.visibility.len());
        self.row_to_edge[row_idx] = None;
        // Extend column data buffer for the new row without generating
        // a spurious [0, create_ts) version chain entry. We do this by
        // writing directly to the column's internal buffer and setting
        // the correct visibility timestamp.
        let mut provided: Vec<Option<&Value>> = vec![None; self.property_schema.len()];
        for (idx, value) in positioned {
            // First occurrence wins, matching the historical name-matched
            // behavior where each column took its first matching value.
            if provided[*idx].is_none() {
                provided[*idx] = Some(value);
            }
        }
        for (i, slot) in provided.into_iter().enumerate() {
            let col = &mut self.property_columns[i];
            match slot {
                Some(v) => {
                    // Value provided: versioned write with the given value
                    col.set_versioned(row_idx, Some(v), create_ts)?;
                }
                None => {
                    // No value provided: use default value if available, otherwise None
                    let default_val = self.property_schema[i].default_value.clone();
                    col.set_with_timestamp(row_idx, default_val.as_ref(), create_ts)?;
                }
            }
            self.mark_column_dirty_at(i);
        }
        Ok(row_idx)
    }

    /// Release a row back to the free list without leaving an orphan.
    ///
    /// Clears the visibility stamp so the slot is skipped by reads and GC
    /// scans, drops the edge mapping via the reverse index, and queues the
    /// slot for reuse. Reused slots are fully overwritten by `allocate_row`.
    /// Idempotent: releasing an already-free or out-of-range row is a no-op.
    /// Slots that were never used are never admitted to the free list.
    pub fn release_row(&mut self, row_idx: usize) {
        if row_idx >= self.visibility.len() {
            return;
        }
        let virgin =
            self.visibility[row_idx].create_ts == 0 && self.visibility[row_idx].delete_ts.is_none();
        if virgin {
            if let Some(slot) = self.row_to_edge.get_mut(row_idx) {
                if let Some(edge_id) = slot.take() {
                    self.map_remove(edge_id);
                }
            }
            return;
        }
        self.visibility[row_idx].create_ts = 0;
        self.visibility[row_idx].delete_ts = None;
        self.row_count = self.row_count.saturating_sub(1);
        if let Some(slot) = self.row_to_edge.get_mut(row_idx) {
            if let Some(edge_id) = slot.take() {
                self.map_remove(edge_id);
            }
        }
        // Released rows are virgin by construction, so a second release
        // takes the virgin path above: slots never enter the free list twice
        // and no membership set is needed.
        self.free_list.push(row_idx as u32);
    }

    /// Get the row index for an edge.
    pub fn get_row_for_edge(&self, edge_id: EdgeId) -> Option<usize> {
        self.mapped_row(edge_id)
    }

    /// Remove edge-to-row mapping and return the row index.
    pub fn remove_edge_mapping(&mut self, edge_id: EdgeId) -> Option<usize> {
        if self.inline {
            return None;
        }
        let pos = self.map_remove(edge_id)?;
        if let Some(slot) = self.row_to_edge.get_mut(pos) {
            if *slot == Some(edge_id) {
                *slot = None;
            }
        }
        Some(pos)
    }

    /// Whether the store holds a row mapping for `edge_id`.
    pub fn contains_edge(&self, edge_id: EdgeId) -> bool {
        self.mapped_row(edge_id).is_some()
    }

    /// Iterate over all edge->row mappings in slot order (for compaction).
    pub fn edge_mappings(&self) -> impl Iterator<Item = (EdgeId, u32)> + '_ {
        self.edge_map_segments
            .iter()
            .enumerate()
            .filter_map(|(seg, segment)| {
                segment.as_ref().map(|values| {
                    let base = seg * Self::EDGE_MAP_SEGMENT_ROWS;
                    values.iter().enumerate().filter_map(move |(off, row)| {
                        (*row != UNMAPPED_ROW).then_some((EdgeId((base + off) as u64), *row))
                    })
                })
            })
            .flatten()
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edge_mappings().map(|(edge_id, _)| edge_id)
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Segment granularity for sparse map scans. One segment covers this
    /// many edge ids; empty segments are skipped by segment-aware scans so
    /// sparse id ranges never pay a full dense walk.
    pub const EDGE_MAP_SEGMENT_ROWS: usize = 1024;

    /// Ids of segments holding at least one live mapping.
    pub fn nonempty_map_segments(&self) -> Vec<usize> {
        self.edge_map_segments
            .iter()
            .enumerate()
            .filter_map(|(seg, segment)| {
                segment
                    .as_ref()
                    .and_then(|values| values.iter().any(|row| *row != UNMAPPED_ROW).then_some(seg))
            })
            .collect()
    }

    /// Mappings within one segment, for segment-skipping scans.
    pub fn edge_mappings_in_segment(
        &self,
        segment: usize,
    ) -> impl Iterator<Item = (EdgeId, u32)> + '_ {
        let base = segment * Self::EDGE_MAP_SEGMENT_ROWS;
        self.edge_map_segments
            .get(segment)
            .and_then(|segment| segment.as_ref())
            .map(|values| {
                values.iter().enumerate().filter_map(move |(off, row)| {
                    (*row != UNMAPPED_ROW).then_some((EdgeId((base + off) as u64), *row))
                })
            })
            .into_iter()
            .flatten()
    }

    /// Sparse map memory in bytes, for observability.
    pub fn edge_map_memory_bytes(&self) -> usize {
        self.edge_map_segments
            .iter()
            .filter(|segment| segment.is_some())
            .count()
            * Self::EDGE_MAP_SEGMENT_ROWS
            * std::mem::size_of::<u32>()
    }
}
