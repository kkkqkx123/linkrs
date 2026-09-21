use super::{CsrWithProperties, RowVisibility, UNMAPPED_ROW};
use graphdb_core::types::{EdgeId, Timestamp, INVALID_EDGE_ID};
use graphdb_core::{StorageError, StorageResult, Value};

impl CsrWithProperties {
    /// Row mapped to `edge_id`, or `None` for unmapped ids.
    ///
    /// Edge ids are table-allocated dense values, so this is one bounds
    /// check plus one indexed read with no hashing.
    pub(crate) fn mapped_row(&self, edge_id: EdgeId) -> Option<usize> {
        let pos = *self.edge_to_row.get(edge_id.0 as usize)?;
        (pos != UNMAPPED_ROW).then_some(pos as usize)
    }

    /// Record the `edge_id` to `row_idx` mapping, growing the dense map.
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
        if slot >= self.edge_to_row.len() {
            self.edge_to_row.resize(slot + 1, UNMAPPED_ROW);
        }
        if self.edge_to_row[slot] == UNMAPPED_ROW {
            self.edge_map_len += 1;
        }
        self.edge_to_row[slot] = row_idx as u32;
        Ok(())
    }

    /// Drop the mapping for `edge_id`, returning its former row.
    pub(crate) fn map_remove(&mut self, edge_id: EdgeId) -> Option<usize> {
        let slot = *self.edge_to_row.get(edge_id.0 as usize)?;
        if slot == UNMAPPED_ROW {
            return None;
        }
        self.edge_to_row[edge_id.0 as usize] = UNMAPPED_ROW;
        self.edge_map_len = self.edge_map_len.saturating_sub(1);
        self.truncate_unmapped_tail();
        Some(slot as usize)
    }

    /// Release trailing unmapped slots so churned id ranges never pin memory.
    fn truncate_unmapped_tail(&mut self) {
        while self.edge_to_row.last() == Some(&UNMAPPED_ROW) {
            self.edge_to_row.pop();
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

    /// Associate an existing row index with an edge id.
    pub fn associate_edge(&mut self, edge_id: EdgeId, row_idx: usize) {
        self.map_insert(edge_id, row_idx)
            .expect("associated edge ids are table-allocated dense values");
        self.ensure_row_aux_len(row_idx + 1);
        self.row_to_edge[row_idx] = Some(edge_id);
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

    /// Iterate over all edge->row mappings (for compaction).
    pub fn edge_mappings(&self) -> impl Iterator<Item = (EdgeId, u32)> + '_ {
        self.edge_to_row
            .iter()
            .enumerate()
            .filter(|(_, row)| **row != UNMAPPED_ROW)
            .map(|(slot, row)| (EdgeId(slot as u64), *row))
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edge_to_row
            .iter()
            .enumerate()
            .filter(|(_, row)| **row != UNMAPPED_ROW)
            .map(|(slot, _)| EdgeId(slot as u64))
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }
}
