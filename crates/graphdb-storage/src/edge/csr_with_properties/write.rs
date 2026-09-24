use super::CsrWithProperties;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

impl CsrWithProperties {
    /// Insert properties for an edge and associate the row with `edge_id`.
    pub fn insert_for_edge(
        &mut self,
        edge_id: EdgeId,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<()> {
        self.reject_inline()?;
        let row_idx = self.allocate_row(values, create_ts)?;
        self.map_insert(edge_id, row_idx)?;
        self.ensure_row_aux_len(row_idx + 1);
        self.row_to_edge[row_idx] = Some(edge_id);
        Ok(())
    }

    /// Insert properties for an edge from pre-resolved column positions.
    ///
    /// Hot write-path entry: the caller resolved every column once through
    /// the schema index, so this performs no name lookup, string clone or
    /// string comparison per edge. Positions must be store column positions;
    /// out-of-range entries fail loudly through `allocate_row_at`.
    pub fn insert_for_edge_at(
        &mut self,
        edge_id: EdgeId,
        positioned: &[(usize, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<()> {
        self.reject_inline()?;
        let row_idx = self.allocate_row_at(positioned, create_ts)?;
        self.map_insert(edge_id, row_idx)?;
        self.ensure_row_aux_len(row_idx + 1);
        self.row_to_edge[row_idx] = Some(edge_id);
        Ok(())
    }

    /// Edge-aware property update: lookup row via `edge_id`.
    pub fn set_property_for_edge(
        &mut self,
        edge_id: EdgeId,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        self.reject_inline()?;
        let pos = self
            .mapped_row(edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        self.set_property_at_row(pos, name, value, ts)
    }

    pub fn set_property_by_id_for_edge(
        &mut self,
        edge_id: EdgeId,
        prop_id: crate::types::PropertyId,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        self.reject_inline()?;
        let pos = self
            .mapped_row(edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        let idx = self
            .prop_id_index
            .get(&(prop_id.0 as i32))
            .copied()
            .ok_or_else(|| StorageError::column_not_found(format!("prop_id={}", prop_id.0)))?;
        if pos >= self.visibility.len() || self.visibility[pos].create_ts == 0 {
            return Err(StorageError::invalid_offset(pos as u32));
        }
        let col = &mut self.property_columns[idx];
        col.set_versioned(pos, value.as_ref(), ts)?;
        self.mark_column_dirty_at(idx);
        Ok(())
    }

    pub fn set_property_at_row(
        &mut self,
        row_idx: usize,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(row_idx as u32));
        }
        let col_idx = self
            .column_index
            .get(name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let col = &mut self.property_columns[col_idx];
        col.set_versioned(row_idx, value.as_ref(), ts)?;
        self.mark_column_dirty_at(col_idx);
        Ok(())
    }

    /// Mark one column position dirty without any name lookup or allocation.
    /// Write paths resolve the position once through the schema indexes and
    /// mark through this.
    pub(crate) fn mark_column_dirty_at(&mut self, idx: usize) {
        self.dirty_columns.insert(idx);
    }
}
