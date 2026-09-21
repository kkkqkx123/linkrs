//! CSR with Properties — ladybug-style columnar storage.
//!
//! Properties are stored in parallel column arrays keyed by `EdgeId` through
//! the edge-to-row map. The topology CSR owns the only adjacency index; this
//! store keeps no per-vertex offsets, lengths, or append heads.
//!
//! This implementation uses `Column` (continuous arrays) instead of
//! `HashMap<u32, Value>` for cache-friendly scans and lower memory overhead.
//!
//! Visibility authority lives in `MVCCManager` (`edge_timestamps`): these CSR
//! row stamps are only a physical projection kept in sync on the write path
//! for garbage collection. They must never decide query visibility alone.

use std::collections::{HashMap, HashSet};

use graphdb_core::types::{EdgeId, Timestamp, INVALID_EDGE_ID};
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::edge::property_schema::PropertySchema;
use crate::vertex::column::Column;

/// Row visibility for MVCC.
///
/// Physical projection of the version authority for collection only.
/// Every visibility decision delegates to the single
/// [`crate::mvcc_visibility::Visibility`] predicate so this copy can never
/// drift into a second time comparison.
#[derive(Debug, Clone, Copy)]
struct RowVisibility {
    create_ts: Timestamp,
    delete_ts: Option<Timestamp>,
}

impl RowVisibility {
    fn new(create_ts: Timestamp) -> Self {
        Self {
            create_ts,
            delete_ts: None,
        }
    }

    #[cfg(test)]
    fn is_visible_at(&self, query_ts: Timestamp) -> bool {
        crate::mvcc_visibility::Visibility::is_visible(query_ts, self.create_ts, self.delete_ts)
    }

    fn mark_deleted(&mut self, ts: Timestamp) {
        if self.delete_ts.is_none() {
            self.delete_ts = Some(ts);
        }
    }
}

/// Columnar property storage keyed by edge id.
///
/// Every edge owns exactly one row, including edges without properties.
/// Row identity is the edge-to-row map; there is no per-vertex addressing.
///
/// Memory and persistence semantics: property before-images (row version
/// chains) stay memory-only; `dump` serializes current values plus row
/// visibility, per-column stable identifiers, encoding choices and refreshed
/// statistics. A reload restores plain values first, then re-applies the
/// recorded encodings and statistics, so encoded scans survive checkpoints.
/// Attribute time travel is therefore valid within a checkpoint epoch.
/// Row slot holding no edge mapping inside the dense edge map.
const UNMAPPED_ROW: u32 = u32::MAX;

/// Exported property row: `(create_ts, delete_ts, per-column values)`.
pub type ExportedRow = (Timestamp, Option<Timestamp>, Vec<(String, Option<Value>)>);

#[derive(Debug, Clone)]
pub struct CsrWithProperties {
    property_schema: Vec<PropertySchema>,
    property_columns: Vec<Column>,
    /// Column position by name, rebuilt on every schema mutation so hot
    /// paths never scan the schema linearly.
    column_index: HashMap<String, usize>,
    /// Column position by stable identifier, rebuilt with the name index.
    /// Undo parameters keyed by id resolve through this instead of scanning.
    prop_id_index: HashMap<i32, usize>,
    visibility: Vec<RowVisibility>,
    /// Dense edge-to-row map indexed by the table-allocated edge id.
    /// Edge ids are monotonic per table, so direct indexing replaces the
    /// former hash lookup; unmapped ids hold `UNMAPPED_ROW`.
    edge_to_row: Vec<u32>,
    /// Live mapping count, maintained alongside the dense map.
    edge_map_len: usize,
    /// Reverse index for O(1) row-to-edge lookup. Authoritative with
    /// `edge_to_row`; rebuilt on load, never persisted separately.
    row_to_edge: Vec<Option<EdgeId>>,
    free_list: Vec<u32>,
    row_count: usize,
    /// Column positions mutated since the last stats refresh or checkpoint.
    /// Drives per-column stats refresh so clean columns never pay recompute.
    /// Positions shift on schema mutation; the schema-mutating methods remap
    /// this set together with the schema.
    dirty_columns: HashSet<usize>,
    /// Stable column identifier allocator. Never reused or reassigned so
    /// stored undo parameters keyed by id stay valid across column drops.
    next_prop_id: i32,
    /// Inline-form marker: the owning table stores its single scalar in the
    /// CSR value column, so this store keeps only the schema and the
    /// name/id indexes. Every row operation fails instead of forking a
    /// second property truth.
    inline: bool,
}

impl CsrWithProperties {
    pub fn new(property_schema: Vec<PropertySchema>) -> Self {
        let mut property_columns = Vec::with_capacity(property_schema.len());
        for schema in &property_schema {
            let col = Column::new(
                schema.name.clone(),
                schema.prop_id,
                schema.data_type.clone(),
                schema.nullable,
            );
            property_columns.push(col);
        }
        let next_prop_id = property_schema
            .iter()
            .map(|s| s.prop_id)
            .max()
            .unwrap_or(-1)
            .saturating_add(1)
            .max(property_schema.len() as i32);
        let mut store = Self {
            property_schema,
            property_columns,
            column_index: HashMap::new(),
            prop_id_index: HashMap::new(),
            visibility: Vec::new(),
            edge_to_row: Vec::new(),
            edge_map_len: 0,
            row_to_edge: Vec::new(),
            free_list: Vec::new(),
            row_count: 0,
            dirty_columns: HashSet::new(),
            next_prop_id,
            inline: false,
        };
        store.rebuild_schema_indexes();
        store
    }

    /// Schema-only stub for the inline record forms.
    ///
    /// Keeps the schema with its name/id indexes for validation and WAL
    /// naming, but holds no rows: every row read or write fails loudly so a
    /// missed dispatch can never fork a second property truth beside the
    /// CSR value column.
    pub fn inline_stub(property_schema: Vec<PropertySchema>) -> Self {
        let mut store = Self::new(property_schema);
        store.inline = true;
        store
    }

    /// Whether this store is a schema-only inline stub.
    pub fn is_inline_stub(&self) -> bool {
        self.inline
    }

    fn reject_inline(&self) -> StorageResult<()> {
        if self.inline {
            return Err(StorageError::invalid_operation(
                "columnar property access on an inline-form table".to_string(),
            ));
        }
        Ok(())
    }

    /// Rebuild the column position indexes after a schema mutation.
    fn rebuild_schema_indexes(&mut self) {
        self.column_index.clear();
        self.prop_id_index.clear();
        for (idx, schema) in self.property_schema.iter().enumerate() {
            self.column_index.insert(schema.name.clone(), idx);
            self.prop_id_index.insert(schema.prop_id, idx);
        }
    }

    /// Column name for one stable identifier, if the column exists.
    pub fn column_name_by_prop_id(&self, prop_id: i32) -> Option<&str> {
        self.prop_id_index
            .get(&prop_id)
            .and_then(|&idx| self.property_schema.get(idx))
            .map(|schema| schema.name.as_str())
    }

    /// Row mapped to `edge_id`, or `None` for unmapped ids.
    ///
    /// Edge ids are table-allocated dense values, so this is one bounds
    /// check plus one indexed read with no hashing.
    fn mapped_row(&self, edge_id: EdgeId) -> Option<usize> {
        let pos = *self.edge_to_row.get(edge_id.0 as usize)?;
        (pos != UNMAPPED_ROW).then_some(pos as usize)
    }

    /// Record the `edge_id` to `row_idx` mapping, growing the dense map.
    ///
    /// Rejects the unassignable gap sentinel explicitly: gap slots never
    /// own property rows.
    fn map_insert(&mut self, edge_id: EdgeId, row_idx: usize) -> StorageResult<()> {
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
    fn map_remove(&mut self, edge_id: EdgeId) -> Option<usize> {
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

    fn ensure_row_aux_len(&mut self, len: usize) {
        if self.row_to_edge.len() < len {
            self.row_to_edge.resize(len, None);
        }
    }

    /// Mark one column position dirty without any name lookup or allocation.
    /// Write paths resolve the position once through the schema indexes and
    /// mark through this.
    fn mark_column_dirty_at(&mut self, idx: usize) {
        self.dirty_columns.insert(idx);
    }

    /// Clear per-column dirt after a successful checkpoint.
    pub fn clear_dirty_columns(&mut self) {
        self.dirty_columns.clear();
    }

    /// Whether any property column changed since the last checkpoint.
    pub fn has_dirty_columns(&self) -> bool {
        !self.dirty_columns.is_empty()
    }

    pub fn property_schema(&self) -> &[PropertySchema] {
        &self.property_schema
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Allocate a new row and populate it with the given values.
    /// Returns the row index (0-based).
    fn allocate_row(
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
    fn allocate_row_at(
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

    /// Read the property row for `edge_id` at `query_ts`, decoding only the
    /// projected columns.
    ///
    /// `projection` selects which columns to decode: `None` decodes every
    /// column, `Some(&[])` decodes none (topology-only read). Unknown names
    /// are skipped. Visibility is still enforced: an invisible edge yields
    /// `None`, a visible one yields `Some` (possibly empty).
    /// Test-only row-stamp filtered read; production uses physical read plus authority gate.
    #[cfg(test)]
    pub fn get_projected_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let pos = self.mapped_row(edge_id)?;
        let vis = self.visibility.get(pos)?;
        if !vis.is_visible_at(query_ts) {
            return None;
        }
        match projection {
            None => Some(
                self.property_schema
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let v = self.property_columns[i].get_at_ts(pos, query_ts);
                        (s.name.clone(), v)
                    })
                    .collect(),
            ),
            Some(names) => {
                if names.is_empty() {
                    return Some(Vec::new());
                }
                Some(
                    self.property_schema
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| names.iter().any(|n| n == &s.name))
                        .map(|(i, s)| {
                            let v = self.property_columns[i].get_at_ts(pos, query_ts);
                            (s.name.clone(), v)
                        })
                        .collect(),
                )
            }
        }
    }

    /// Test-only row-stamp filtered read; production uses physical read plus authority gate.
    #[cfg(test)]
    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Option<Vec<(String, Option<Value>)>> {
        self.get_projected_by_edge_id(edge_id, query_ts, None)
    }

    /// Physical property projection without row visibility filtering.
    ///
    /// Callers must decide visibility through the version authority first;
    /// row stamps exist only for collection. Returns `None` only when the
    /// edge has no row mapping.
    pub fn get_projected_physical_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        if self.inline {
            return None;
        }
        let pos = self.mapped_row(edge_id)?;
        if pos >= self.visibility.len() {
            return None;
        }
        match projection {
            None => Some(
                self.property_schema
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let v = self.property_columns[i].get_at_ts(pos, query_ts);
                        (s.name.clone(), v)
                    })
                    .collect(),
            ),
            Some(names) => {
                if names.is_empty() {
                    return Some(Vec::new());
                }
                Some(
                    self.property_schema
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| names.iter().any(|n| n == &s.name))
                        .map(|(i, s)| {
                            let v = self.property_columns[i].get_at_ts(pos, query_ts);
                            (s.name.clone(), v)
                        })
                        .collect(),
                )
            }
        }
    }

    /// Read non-nullable properties for an edge by its EdgeId (no MVCC filtering).
    pub fn read_properties_by_edge_id(&self, edge_id: EdgeId) -> Option<Vec<(String, Value)>> {
        if self.inline {
            return None;
        }
        let pos = self.mapped_row(edge_id)?;
        let result: Vec<(String, Value)> = self
            .property_schema
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let v = self.property_columns[i].get(pos)?;
                Some((s.name.clone(), v))
            })
            .collect();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Column index for one property name.
    fn column_index(&self, name: &str) -> Option<usize> {
        self.column_index.get(name).copied()
    }

    /// Snapshot value of one column cell for pushdown filtering.
    ///
    /// Reads through the version chain at `query_ts`; a `None` return means
    /// null at that snapshot, expressed through the column null bitmap rather
    /// than a materialized record.
    fn pushdown_cell(&self, row: usize, column: usize, query_ts: Timestamp) -> Option<Value> {
        self.property_columns.get(column)?.get_at_ts(row, query_ts)
    }

    /// Whether one edge matches every pushed predicate at `query_ts`.
    ///
    /// Column-scan layer: only predicate columns are read, each through its
    /// null bitmap, and no intermediate record is materialized. A missing
    /// column, a missing row mapping or a null cell never matches, mirroring
    /// the query NULL semantics where comparisons against NULL are false.
    pub fn matches_predicates_for_edge(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        predicates: &[crate::cursor::ScanPredicate],
    ) -> bool {
        if predicates.is_empty() {
            return true;
        }
        let Some(row) = self.mapped_row(edge_id) else {
            return false;
        };
        for predicate in predicates {
            let Some(column) = self.column_index(predicate.column()) else {
                return false;
            };
            let Some(value) = self.pushdown_cell(row, column, query_ts) else {
                return false;
            };
            if !predicate.matches_value(&value) {
                return false;
            }
        }
        true
    }

    /// Filter edge ids by pushed predicates at the column-scan layer.
    ///
    /// Attribute equality and range predicates filter row numbers first;
    /// callers look up topology only for the returned hits. Nulls use bitmap
    /// semantics throughout and no intermediate records are materialized.
    /// `candidates` bounds the scan when the caller already holds row
    /// numbers; `None` scans every mapped edge.
    pub fn filter_edge_ids_by_predicates(
        &self,
        predicates: &[crate::cursor::ScanPredicate],
        query_ts: Timestamp,
        candidates: Option<&[EdgeId]>,
    ) -> Vec<EdgeId> {
        if predicates.is_empty() {
            return candidates.map_or_else(
                || {
                    self.edge_to_row
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| **row != UNMAPPED_ROW)
                        .map(|(slot, _)| EdgeId(slot as u64))
                        .collect()
                },
                <[EdgeId]>::to_vec,
            );
        }
        let resolved: Vec<(usize, &crate::cursor::ScanPredicate)> = predicates
            .iter()
            .map(|predicate| (self.column_index(predicate.column()), predicate))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|(index, predicate)| index.map(|column| (column, predicate)))
            .collect();
        if resolved.len() != predicates.len() {
            return Vec::new();
        }
        match candidates {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|edge_id| {
                    let Some(row) = self.mapped_row(*edge_id) else {
                        return false;
                    };
                    resolved.iter().all(|(column, predicate)| {
                        self.pushdown_cell(row, *column, query_ts)
                            .is_some_and(|value| predicate.matches_value(&value))
                    })
                })
                .collect(),
            None => self
                .edge_to_row
                .iter()
                .enumerate()
                .filter_map(|(slot, row)| {
                    if *row == UNMAPPED_ROW {
                        return None;
                    }
                    let row = *row as usize;
                    resolved
                        .iter()
                        .all(|(column, predicate)| {
                            self.pushdown_cell(row, *column, query_ts)
                                .is_some_and(|value| predicate.matches_value(&value))
                        })
                        .then_some(EdgeId(slot as u64))
                })
                .collect(),
        }
    }

    pub fn mark_deleted(&mut self, edge_id: EdgeId, ts: Timestamp) -> bool {
        if self.inline {
            return false;
        }
        if let Some(pos) = self.mapped_row(edge_id) {
            if let Some(vis) = self.visibility.get_mut(pos) {
                if vis.delete_ts.is_some() {
                    return false;
                }
                vis.mark_deleted(ts);
                return true;
            }
        }
        false
    }

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

    /// Edge-aware bulk property update: lookup row via `edge_id` and update all properties.
    pub fn update_properties_for_edge(
        &mut self,
        edge_id: EdgeId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let pos = self
            .mapped_row(edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        self.update_at_row(pos, properties, ts)
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

    pub fn revert_deletion_for_edge(&mut self, edge_id: EdgeId) -> bool {
        if self.inline {
            return false;
        }
        if let Some(pos) = self.mapped_row(edge_id) {
            return self.revert_deletion_at_row(pos);
        }
        false
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

    pub fn mark_deleted_at_row(&mut self, row_idx: usize, ts: Timestamp) -> StorageResult<()> {
        if row_idx >= self.visibility.len() {
            return Ok(());
        }
        if self.visibility[row_idx].delete_ts.is_some() {
            return Err(StorageError::invalid_operation(
                "record already marked deleted",
            ));
        }
        self.visibility[row_idx].mark_deleted(ts);
        Ok(())
    }

    pub fn is_deleted_at_row(&self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get(row_idx) {
            return vis.delete_ts.is_some();
        }
        false
    }

    pub fn revert_deletion_at_row(&mut self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get_mut(row_idx) {
            if vis.delete_ts.is_some() {
                vis.delete_ts = None;
                return true;
            }
        }
        false
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.column_index.contains_key(name)
    }

    pub fn get_property_id(&self, name: &str) -> Option<crate::types::PropertyId> {
        self.column_index
            .get(name)
            .map(|&idx| crate::types::PropertyId::new(self.property_schema[idx].prop_id as u16))
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<crate::types::PropertyId> {
        if self.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }
        let prop_id = self.next_prop_id;
        self.next_prop_id = self.next_prop_id.saturating_add(1);
        let schema =
            PropertySchema::new(name.clone(), prop_id, data_type.clone()).nullable(nullable);
        self.property_schema.push(schema);
        let mut col = Column::new(name.clone(), prop_id, data_type, nullable);
        let rows = self.visibility.len();
        if rows > 0 {
            col.resize(rows);
        }
        self.property_columns.push(col);
        let idx = self.property_schema.len() - 1;
        self.column_index.insert(name, idx);
        self.mark_column_dirty_at(idx);
        Ok(crate::types::PropertyId::new(prop_id as u16))
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        let idx = self
            .column_index
            .get(name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        self.property_schema.remove(idx);
        self.property_columns.remove(idx);
        self.rebuild_schema_indexes();
        // Positions above the removed column shift down by one; the dirt set
        // moves with them instead of keeping stale positions.
        let mut shifted = HashSet::new();
        for pos in self.dirty_columns.drain() {
            if pos == idx {
                continue;
            }
            shifted.insert(if pos > idx { pos - 1 } else { pos });
        }
        self.dirty_columns = shifted;
        Ok(())
    }

    /// Clone one physical column for the staged drop snapshot.
    pub fn column_cloned(&self, name: &str) -> Option<crate::vertex::column::Column> {
        self.column_index
            .get(name)
            .and_then(|&idx| self.property_columns.get(idx).cloned())
    }

    /// Whether one column holds unrefreshed writes.
    pub fn has_column_dirt(&self, name: &str) -> bool {
        self.column_index
            .get(name)
            .is_some_and(|idx| self.dirty_columns.contains(idx))
    }

    /// Names of columns mutated since the last checkpoint. Drives
    /// dirty-column incremental persistence: clean columns reuse the last
    /// flushed encoding instead of paying re-export and re-encode.
    pub fn dirty_column_names(&self) -> Vec<String> {
        self.dirty_columns
            .iter()
            .filter_map(|idx| self.property_schema.get(*idx))
            .map(|schema| schema.name.clone())
            .collect()
    }

    /// Put back a column removed by a failed drop publish at its exact
    /// schema position, restoring its dirt mark when it had one.
    pub fn restore_property_at(
        &mut self,
        index: usize,
        schema: PropertySchema,
        column: crate::vertex::column::Column,
        had_column_dirt: bool,
    ) {
        let at = index.min(self.property_schema.len());
        self.property_schema.insert(at, schema);
        let at = at.min(self.property_columns.len());
        self.property_columns.insert(at, column);
        self.rebuild_schema_indexes();
        // Positions at or above the restored column shift up by one before
        // the restored dirt mark lands, keeping every position exact.
        let mut shifted = HashSet::new();
        for pos in self.dirty_columns.drain() {
            shifted.insert(if pos >= at { pos + 1 } else { pos });
        }
        self.dirty_columns = shifted;
        if had_column_dirt {
            self.dirty_columns.insert(at);
        }
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if self.has_property(new_name) {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }
        let idx = self
            .column_index
            .get(old_name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;
        self.property_schema[idx].name = new_name.to_string();
        if let Some(col) = self.property_columns.get_mut(idx) {
            col.name = new_name.to_string();
        }
        // Dirt is tracked by position, so a rename moves no marks.
        self.rebuild_schema_indexes();
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

    /// Bulk update properties at a given row index.
    pub fn update_at_row(
        &mut self,
        row_idx: usize,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(row_idx as u32));
        }
        for (name, value) in properties {
            let col_idx = self
                .column_index
                .get(name.as_str())
                .copied()
                .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
            let col = &mut self.property_columns[col_idx];
            col.set_versioned(row_idx, Some(value), ts)?;
            self.mark_column_dirty_at(col_idx);
        }
        Ok(())
    }

    pub fn compaction_stats(&self) -> crate::edge::property_schema::PropertyCompactionStats {
        let tombstone_count = self
            .visibility
            .iter()
            .filter(|v| v.delete_ts.is_some())
            .count();
        let live_records = self
            .visibility
            .iter()
            .filter(|v| v.create_ts != 0 && v.delete_ts.is_none())
            .count();
        let mut reclaimable_bytes = 0usize;
        for v in &self.visibility {
            if v.delete_ts.is_some() {
                reclaimable_bytes += 32 * self.property_schema.len();
            }
        }
        crate::edge::property_schema::PropertyCompactionStats {
            tombstone_count,
            total_records: self.visibility.len(),
            live_records,
            reclaimable_bytes,
        }
    }

    /// Garbage-collect property version-chain entries that no active snapshot
    /// can observe. Returns the total number of before-images removed.
    pub fn gc_property_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        self.property_columns
            .iter_mut()
            .map(|col| col.gc_versions(min_active_snapshot_ts))
            .sum()
    }

    /// Aggregate version-chain statistics across all property columns.
    pub fn property_version_stats(&self) -> crate::vertex::column::mvcc::VersionChainStats {
        let mut total_rows = 0usize;
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for col in &self.property_columns {
            let stats = col.version_chain_stats();
            total_rows = total_rows.max(stats.total_rows);
            total_entries += stats.total_entries;
            max_len = max_len.max(stats.max_len);
            memory_bytes += stats.memory_bytes;
        }
        let avg_len = if total_rows > 0 {
            total_entries as f64 / total_rows as f64
        } else {
            0.0
        };
        crate::vertex::column::mvcc::VersionChainStats {
            total_rows,
            total_entries,
            max_len,
            avg_len,
            memory_bytes,
        }
    }

    pub fn is_schema_fixed_size(&self) -> bool {
        self.property_schema.iter().all(|s| {
            matches!(
                s.data_type,
                DataType::Bool
                    | DataType::SmallInt
                    | DataType::Int
                    | DataType::BigInt
                    | DataType::Float
                    | DataType::Double
            )
        })
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        total += self.visibility.capacity() * std::mem::size_of::<RowVisibility>();
        total += self.edge_to_row.capacity() * std::mem::size_of::<u32>();
        total += self.free_list.capacity() * std::mem::size_of::<u32>();
        total += self.row_to_edge.capacity() * std::mem::size_of::<Option<EdgeId>>();
        total += self.column_index.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());
        for col in &self.property_columns {
            total += col.memory_size();
        }
        total += self.property_schema.len() * std::mem::size_of::<PropertySchema>();
        total
    }

    pub fn dump(&self) -> Vec<u8> {
        // Current-value snapshot plus per-column identity, encoding choice
        // and statistics. Row version chains stay memory-only by design:
        // load restores plain values holding the latest value with the row
        // creation stamp, then re-applies the recorded encodings.
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.visibility.len() as u32).to_le_bytes());
        for vis in &self.visibility {
            buf.extend_from_slice(&vis.create_ts.to_le_bytes());
            if let Some(del) = vis.delete_ts {
                buf.push(1);
                buf.extend_from_slice(&del.to_le_bytes());
            } else {
                buf.push(0);
            }
        }
        buf.extend_from_slice(&(self.row_count as u32).to_le_bytes());
        buf.extend_from_slice(&(self.edge_map_len as u32).to_le_bytes());
        for (slot, pos) in self.edge_to_row.iter().enumerate() {
            if *pos == UNMAPPED_ROW {
                continue;
            }
            buf.extend_from_slice(&(slot as u64).to_le_bytes());
            buf.extend_from_slice(&pos.to_le_bytes());
        }
        buf.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        // Serialize current column values (without version history) keyed by
        // column name, each carrying its stable identifier, its encoding
        // choice and its last refreshed statistics.
        buf.extend_from_slice(&(self.property_columns.len() as u32).to_le_bytes());
        for (idx, col) in self.property_columns.iter().enumerate() {
            // Column name keys the payload to the schema entry on load.
            buf.extend_from_slice(&(col.name.len() as u32).to_le_bytes());
            buf.extend_from_slice(col.name.as_bytes());
            let prop_id = self
                .property_schema
                .get(idx)
                .map(|schema| schema.prop_id)
                .unwrap_or(-1);
            buf.extend_from_slice(&prop_id.to_le_bytes());
            buf.push(col.encoding_type().to_u8());
            let rows = self.visibility.len();
            buf.extend_from_slice(&(rows as u32).to_le_bytes());
            buf.reserve(rows.saturating_mul(16));
            let mut cell_scratch: Vec<u8> = Vec::with_capacity(64);
            for row_idx in 0..rows {
                let val = col.get(row_idx);
                if let Some(v) = val {
                    buf.push(1);
                    cell_scratch.clear();
                    let taken = std::mem::take(&mut cell_scratch);
                    match postcard::to_extend(&v, taken) {
                        Ok(encoded) => {
                            buf.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
                            buf.extend_from_slice(&encoded);
                            cell_scratch = encoded;
                        }
                        Err(_) => {
                            buf.extend_from_slice(&0u32.to_le_bytes());
                            cell_scratch = Vec::with_capacity(64);
                        }
                    }
                } else {
                    buf.push(0);
                }
            }
            match col.stats() {
                Some(stats) => {
                    let mut stats_buf = Vec::new();
                    if stats.serialize_meta(&mut stats_buf).is_ok() {
                        buf.push(1);
                        buf.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                        buf.extend_from_slice(&stats_buf);
                    } else {
                        buf.push(0);
                    }
                }
                None => buf.push(0),
            }
        }
        buf
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        fn need(data: &[u8], offset: usize, len: usize, what: &str) -> StorageResult<()> {
            if data.len().saturating_sub(offset) < len {
                return Err(StorageError::deserialize_error(format!(
                    "properties payload too short for {}",
                    what
                )));
            }
            Ok(())
        }
        if data.is_empty() {
            return Err(StorageError::deserialize_error(
                "properties payload is empty",
            ));
        }
        let mut offset = 0usize;
        need(data, offset, 4, "visibility length")?;
        let vis_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.visibility.clear();
        self.visibility.reserve(vis_len);
        for _ in 0..vis_len {
            need(data, offset, 8, "row creation stamp")?;
            let create = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            need(data, offset, 1, "row deletion flag")?;
            let has_del = data[offset];
            offset += 1;
            let del = if has_del == 1 {
                need(data, offset, 8, "row deletion stamp")?;
                let d = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Some(d)
            } else if has_del == 0 {
                None
            } else {
                return Err(StorageError::deserialize_error(format!(
                    "invalid row deletion flag: {}",
                    has_del
                )));
            };
            self.visibility.push(RowVisibility {
                create_ts: create,
                delete_ts: del,
            });
        }
        need(data, offset, 4, "row count")?;
        self.row_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        need(data, offset, 4, "edge map length")?;
        let map_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.edge_to_row.clear();
        self.edge_map_len = 0;
        // Wire format is unchanged (entry count plus id/row pairs); the dense
        // map is rebuilt from the pairs. Rows must land inside the restored
        // visibility window, otherwise the payload is rejected.
        let vis_len = self.visibility.len();
        for _ in 0..map_len {
            need(data, offset, 12, "edge map entry")?;
            let eid = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            let pos = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            if eid == INVALID_EDGE_ID.0 || (pos as usize) >= vis_len {
                return Err(StorageError::deserialize_error(
                    "edge map entry outside the restored row window",
                ));
            }
            let slot = eid as usize;
            if slot >= self.edge_to_row.len() {
                self.edge_to_row.resize(slot + 1, UNMAPPED_ROW);
            }
            if self.edge_to_row[slot] == UNMAPPED_ROW {
                self.edge_map_len += 1;
            }
            self.edge_to_row[slot] = pos;
        }
        need(data, offset, 4, "free list length")?;
        let free_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.free_list.clear();
        for _ in 0..free_len {
            need(data, offset, 4, "free list entry")?;
            let off = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            self.free_list.push(off);
        }
        need(data, offset, 4, "column count")?;
        {
            let col_count =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let mut seen_names = HashSet::new();
            let mut seen_ids = HashSet::new();
            for _ in 0..col_count {
                need(data, offset, 4, "column name length")?;
                let name_len =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                need(data, offset, name_len, "column name")?;
                let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                offset += name_len;
                if !seen_names.insert(name.clone()) {
                    return Err(StorageError::deserialize_error(format!(
                        "duplicate column in properties payload: {}",
                        name
                    )));
                }
                need(data, offset, 4, "column identifier")?;
                let prop_id = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                if !seen_ids.insert(prop_id) {
                    return Err(StorageError::deserialize_error(format!(
                        "duplicate column identifier in properties payload: {}",
                        prop_id
                    )));
                }
                need(data, offset, 1, "column encoding")?;
                let encoding_tag = data[offset];
                offset += 1;
                if encoding_tag > 6 {
                    return Err(StorageError::deserialize_error(format!(
                        "unknown column encoding tag: {}",
                        encoding_tag
                    )));
                }
                let encoding = crate::encoding::EncodingType::from_u8(encoding_tag);
                need(data, offset, 4, "column row count")?;
                let rows =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                if let Some(&col_idx) = self.column_index.get(&name) {
                    self.property_schema[col_idx].prop_id = prop_id;
                    let col = &mut self.property_columns[col_idx];
                    col.col_id = prop_id;
                    if col.len() < rows {
                        col.resize(rows);
                    }
                    for row_idx in 0..rows {
                        need(data, offset, 1, "column cell flag")?;
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            need(data, offset, 4, "column cell length")?;
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            need(data, offset, vlen, "column cell value")?;
                            let vbytes = &data[offset..offset + vlen];
                            offset += vlen;
                            if let Ok(val) = postcard::from_bytes::<Value>(vbytes) {
                                let _ = col.set(row_idx, Some(&val));
                            }
                        } else if has == 0 {
                            let _ = col.set(row_idx, None);
                        } else {
                            return Err(StorageError::deserialize_error(format!(
                                "invalid column cell flag: {}",
                                has
                            )));
                        }
                    }
                    need(data, offset, 1, "column stats flag")?;
                    let has_stats = data[offset];
                    offset += 1;
                    if has_stats == 1 {
                        need(data, offset, 4, "column stats length")?;
                        let stats_len =
                            u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                as usize;
                        offset += 4;
                        need(data, offset, stats_len, "column stats")?;
                        let stats_bytes = &data[offset..offset + stats_len];
                        offset += stats_len;
                        let mut cursor = stats_bytes;
                        let stats =
                            crate::column_stats::ColumnStats::deserialize_meta(&mut cursor)?;
                        if !cursor.is_empty() {
                            return Err(StorageError::deserialize_error(
                                "unexpected trailing data in column stats".to_string(),
                            ));
                        }
                        if encoding != crate::encoding::EncodingType::None {
                            self.apply_encoding_to_column(&name, encoding, 255)?;
                        }
                        let col = &mut self.property_columns[col_idx];
                        col.set_stats(stats);
                    } else if has_stats == 0 {
                        if encoding != crate::encoding::EncodingType::None {
                            self.apply_encoding_to_column(&name, encoding, 255)?;
                        }
                    } else {
                        return Err(StorageError::deserialize_error(format!(
                            "invalid column stats flag: {}",
                            has_stats
                        )));
                    }
                } else {
                    for _ in 0..rows {
                        need(data, offset, 1, "unknown column cell flag")?;
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            need(data, offset, 4, "unknown column cell length")?;
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            need(data, offset, vlen, "unknown column cell value")?;
                            offset += vlen;
                        } else if has != 0 {
                            return Err(StorageError::deserialize_error(format!(
                                "invalid unknown column cell flag: {}",
                                has
                            )));
                        }
                    }
                    // Unpublished columns keep the abort-on-reload contract:
                    // skip their cells, encoding tag and statistics alike.
                    need(data, offset, 1, "unknown column stats flag")?;
                    let unknown_stats = data[offset];
                    offset += 1;
                    if unknown_stats == 1 {
                        need(data, offset, 4, "unknown column stats length")?;
                        let stats_len =
                            u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                as usize;
                        offset += 4;
                        need(data, offset, stats_len, "unknown column stats")?;
                        offset += stats_len;
                    } else if unknown_stats != 0 {
                        return Err(StorageError::deserialize_error(format!(
                            "invalid unknown column stats flag: {}",
                            unknown_stats
                        )));
                    }
                }
            }
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in properties payload".to_string(),
            ));
        }
        self.rebuild_aux_indexes();
        self.dirty_columns.clear();
        Ok(())
    }

    fn rebuild_aux_indexes(&mut self) {
        self.row_to_edge.clear();
        self.row_to_edge.resize(self.visibility.len(), None);
        for (slot, pos) in self.edge_to_row.iter().enumerate() {
            if *pos == UNMAPPED_ROW {
                continue;
            }
            let idx = *pos as usize;
            if idx < self.row_to_edge.len() {
                self.row_to_edge[idx] = Some(EdgeId(slot as u64));
            }
        }
        self.rebuild_schema_indexes();
        let max_id = self
            .property_schema
            .iter()
            .map(|s| s.prop_id)
            .max()
            .unwrap_or(-1);
        self.next_prop_id = max_id
            .saturating_add(1)
            .max(self.property_schema.len() as i32);
    }

    pub fn reclaim_slots(
        &mut self,
        valid_edge_ids: &HashSet<EdgeId>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut to_reclaim = Vec::new();
        for (idx, vis) in self.visibility.iter().enumerate() {
            if vis.create_ts == 0 {
                continue;
            }
            // O(1) ownership check via the reverse index instead of scanning
            // the full edge map for every row.
            let has_live_edge = self
                .row_to_edge
                .get(idx)
                .and_then(|slot| *slot)
                .is_some_and(|eid| valid_edge_ids.contains(&eid));
            if has_live_edge {
                continue;
            }
            if let Some(del_ts) = vis.delete_ts {
                // Exclusive waterfront: deletable exactly when invisible to
                // every snapshot at or past the cutoff.
                if crate::mvcc_visibility::Visibility::is_gc_eligible(del_ts, retention_bound) {
                    to_reclaim.push(idx);
                }
            }
        }
        if to_reclaim.is_empty() {
            return 0;
        }
        for &idx in &to_reclaim {
            self.visibility[idx].create_ts = 0;
            self.visibility[idx].delete_ts = None;
            if let Some(taken) = self.row_to_edge.get_mut(idx).and_then(|slot| slot.take()) {
                self.map_remove(taken);
            }
            // Reclaimed rows are virgin after the stamp clear, so the free
            // list admits each of them exactly once with no membership set.
            self.free_list.push(idx as u32);
            self.row_count = self.row_count.saturating_sub(1);
        }
        to_reclaim.len()
    }

    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        let col = self
            .column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))?;
        let mut min: Option<Value> = None;
        let mut max: Option<Value> = None;
        for zone in col.zone_maps() {
            if let Some(v) = &zone.min {
                match &min {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Greater => {}
                    _ => min = Some(v.clone()),
                }
            }
            if let Some(v) = &zone.max {
                match &max {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Less => {}
                    _ => max = Some(v.clone()),
                }
            }
        }
        let persisted = col.stats();
        let null_count = persisted.map(|s| s.null_count);
        let (distinct_count, hll) = match persisted.and_then(|s| s.hll.clone()) {
            Some(h) => {
                let est = h.estimate();
                (Some(est), persisted.and_then(|s| s.hll.clone()))
            }
            None => (None, None),
        };
        Some(crate::stats_reader::ColumnStatsSnapshot {
            row_count: self.row_count as u64,
            null_count,
            distinct_count,
            hll,
            min_value: min,
            max_value: max,
        })
    }

    /// Encoding applied to one property column, if the column exists.
    pub fn column_encoding_type(&self, column: &str) -> Option<crate::encoding::EncodingType> {
        self.column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))
            .map(|c| c.encoding_type())
    }

    /// Apply one encoding to a single property column.
    ///
    /// Chunked columns take the chunk-local path so point updates keep
    /// decoding only the affected chunk; plain columns dispatch by type.
    /// Empty columns are a no-op. Unknown columns are an explicit error.
    pub fn apply_encoding_to_column(
        &mut self,
        column: &str,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        let idx = self
            .column_index
            .get(column)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(column.to_string()))?;
        let col = &mut self.property_columns[idx];
        if col.is_empty() {
            return Ok(());
        }
        if col.has_chunks() {
            col.apply_encoding_to_chunks(encoding_type, fsst_max_symbols)?;
            if let Some(schema) = self.property_schema.get_mut(idx) {
                schema.encoding_type = col.encoding_type();
            }
            return Ok(());
        }
        match encoding_type {
            crate::encoding::EncodingType::Fsst => {
                col.apply_fsst_encoding(fsst_max_symbols)?;
            }
            crate::encoding::EncodingType::Dictionary => {
                col.apply_dictionary_encoding()?;
            }
            crate::encoding::EncodingType::Rle => {
                col.apply_rle_encoding()?;
            }
            crate::encoding::EncodingType::BitPacking => {
                col.apply_bitpacking_encoding()?;
            }
            crate::encoding::EncodingType::Alp => {
                col.apply_alp_encoding()?;
            }
            crate::encoding::EncodingType::Constant => {
                col.apply_constant_encoding()?;
            }
            crate::encoding::EncodingType::None => {}
        }
        if let Some(schema) = self.property_schema.get_mut(idx) {
            schema.encoding_type = col.encoding_type();
        }
        Ok(())
    }

    /// Select and apply one encoding per property column from current values.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between runs
    /// by design so everyday writes never pay re-encoding. Returns the number
    /// of columns that received an encoding.
    pub fn auto_encode_properties(&mut self) -> usize {
        let selector = crate::encoding::EncodingSelector::default();
        let mut encoded = 0usize;
        for idx in 0..self.property_columns.len() {
            let data_type = self.property_columns[idx].data_type.clone();
            let values: Vec<Option<Value>> = (0..self.property_columns[idx].len())
                .map(|row| self.property_columns[idx].get(row))
                .collect();
            if values.is_empty() {
                continue;
            }
            let selected = selector.select_for_column(&data_type, &values);
            if selected == crate::encoding::EncodingType::None {
                continue;
            }
            let name = self.property_columns[idx].name.clone();
            if self
                .apply_encoding_to_column(name.as_str(), selected, 255)
                .is_ok()
            {
                encoded += 1;
            }
        }
        encoded
    }

    /// Recompute persisted per-column statistics from flush buffers.
    ///
    /// Called before the property file is serialized so statistics follow the
    /// checkpoint instead of drifting. Only columns marked dirty are
    /// recomputed; clean columns keep their persisted statistics. Columns
    /// that fail to compute keep their previous statistics.
    pub fn refresh_column_stats(&mut self) {
        if self.dirty_columns.is_empty() {
            for col in &mut self.property_columns {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
            return;
        }
        // Dirt marks stay until the checkpoint clears them: stats refresh
        // must not consume the marks that drive dirty-column persistence.
        let dirty: Vec<usize> = self.dirty_columns.iter().copied().collect();
        for idx in dirty {
            if let Some(col) = self.property_columns.get_mut(idx) {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
        }
    }

    /// Refresh statistics for one column only. Used by column-level
    /// checkpoint follow-up when only a subset changed.
    pub fn refresh_column_stats_for(&mut self, column: &str) {
        if let Some(&idx) = self.column_index.get(column) {
            if let Some(col) = self.property_columns.get_mut(idx) {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
        }
    }

    /// Backfill one column with a default value on every existing row.
    ///
    /// Used by the staged add-column fill step. Unknown columns are an
    /// explicit error; a single row failure aborts with the error.
    pub fn backfill_column(&mut self, column: &str, default: &Value) -> StorageResult<()> {
        let rows = self.visibility.len();
        let idx = self
            .column_index
            .get(column)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(column.to_string()))?;
        let col = &mut self.property_columns[idx];
        for row in 0..rows {
            col.set(row, Some(default))?;
        }
        self.mark_column_dirty_at(idx);
        Ok(())
    }

    /// Restore stable column identifiers from shard payloads after a merge.
    ///
    /// Shard dumps carry the flushed identifiers; the fresh merge target
    /// starts from dense indexes, so identifiers are re-applied here to keep
    /// undo parameters keyed by id valid across checkpoints. Unknown names
    /// are skipped. The allocator moves past the maximum restored id.
    pub fn restore_prop_ids(&mut self, ids: &HashMap<String, i32>) {
        for (idx, schema) in self.property_schema.iter_mut().enumerate() {
            if let Some(id) = ids.get(&schema.name) {
                schema.prop_id = *id;
                if let Some(col) = self.property_columns.get_mut(idx) {
                    col.col_id = *id;
                }
            }
        }
        let max_id = self
            .property_schema
            .iter()
            .map(|s| s.prop_id)
            .max()
            .unwrap_or(-1);
        self.next_prop_id = max_id
            .saturating_add(1)
            .max(self.property_schema.len() as i32);
    }

    /// Stable identifier for one column in this shard, if present.
    pub fn prop_id_of(&self, name: &str) -> Option<i32> {
        self.column_index
            .get(name)
            .map(|&idx| self.property_schema[idx].prop_id)
    }

    /// Export one edge row for per-group property sharding.
    ///
    /// Returns the row visibility plus current values for every schema
    /// column (nulls as `None`). Unknown edges yield `None`. The export
    /// carries plain values only; version history stays memory-only and is
    /// collapsed on checkpoint, matching the whole-file dump contract.
    pub fn export_row(&self, edge_id: EdgeId) -> Option<ExportedRow> {
        let pos = self.mapped_row(edge_id)?;
        let vis = *self.visibility.get(pos)?;
        let values = self
            .property_schema
            .iter()
            .enumerate()
            .map(|(idx, schema)| {
                let current = self.property_columns.get(idx).and_then(|col| col.get(pos));
                (schema.name.clone(), current)
            })
            .collect();
        Some((vis.create_ts, vis.delete_ts, values))
    }

    /// Import one exported row into this store for shard merge.
    ///
    /// Unknown columns in `values` are skipped so shards written under an
    /// older published schema still merge; missing columns keep their
    /// defaults via the fresh insert path. Nulls are materialized
    /// explicitly so a shard null never becomes a default on merge.
    pub fn import_row(
        &mut self,
        edge_id: EdgeId,
        create_ts: Timestamp,
        delete_ts: Option<Timestamp>,
        values: &[(String, Option<Value>)],
    ) -> StorageResult<()> {
        if self.mapped_row(edge_id).is_some() {
            return Ok(());
        }
        let mut present: Vec<(usize, Value)> = Vec::new();
        let mut nulls: Vec<String> = Vec::new();
        for (name, opt) in values {
            let Some(&idx) = self.column_index.get(name.as_str()) else {
                continue;
            };
            match opt {
                Some(value) => present.push((idx, value.clone())),
                None => nulls.push(name.clone()),
            }
        }
        self.insert_for_edge_at(edge_id, &present, create_ts)?;
        for name in nulls {
            let _ = self.set_property_for_edge(edge_id, &name, None, create_ts);
        }
        if let Some(delete) = delete_ts {
            let _ = self.mark_deleted(edge_id, delete);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::EncodingType;
    use graphdb_core::DataType;

    fn schema() -> Vec<PropertySchema> {
        vec![
            PropertySchema::new("weight".to_string(), 0, DataType::Double),
            PropertySchema::new("label".to_string(), 1, DataType::String).nullable(true),
        ]
    }

    #[test]
    fn edge_id_access() {
        let mut csr = CsrWithProperties::new(schema());
        let eid0 = EdgeId(1);
        let eid1 = EdgeId(2);
        csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.5))], 10)
            .unwrap();
        csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.5))], 10)
            .unwrap();
        let p0 = csr.get_by_edge_id(eid0, 10).unwrap();
        assert!(p0
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.5))));
        let by_id = csr.get_by_edge_id(eid1, 10).unwrap();
        assert!(by_id
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.5))));
        assert_eq!(csr.row_count(), 2);
    }

    #[test]
    fn positioned_insert_matches_named_insert() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(11);
        // Column positions follow schema order: weight=0, label=1.
        csr.insert_for_edge_at(eid, &[(0, Value::Double(4.5))], 10)
            .unwrap();
        let got = csr.get_by_edge_id(eid, 10).unwrap();
        assert!(got
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(4.5))));

        // Out-of-range positions fail instead of writing the wrong column.
        assert!(csr
            .insert_for_edge_at(EdgeId(12), &[(9, Value::Double(1.0))], 10)
            .is_err());

        // Unknown names on the named path resolve to no column, so the
        // non-nullable column keeps its missing default and fails loudly.
        assert!(csr
            .insert_for_edge(EdgeId(13), &[("nope".to_string(), Value::Double(1.0))], 10)
            .is_err());
    }

    #[test]
    fn visibility() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(99);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        assert!(csr.get_by_edge_id(eid, 99).is_none());
        assert!(csr.get_by_edge_id(eid, 100).is_some());
        csr.mark_deleted(eid, 150);
        assert!(csr.get_by_edge_id(eid, 149).is_some());
        assert!(csr.get_by_edge_id(eid, 150).is_none());
    }

    #[test]
    fn columnar_repeatable_read() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(42);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        // Snapshot read at 150 still observes the before-image (RepeatableRead).
        let old = csr.get_by_edge_id(eid, 150).unwrap();
        assert!(old
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.0))));
        // Newer readers observe the latest write.
        let got = csr.get_by_edge_id(eid, 250).unwrap();
        assert!(got
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    }

    #[test]
    fn columnar_property_version_gc_keeps_visible_snapshots() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(7);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(3.0)), 300)
            .unwrap();
        // Entries ending at or before the cutoff are eligible; the chain
        // covering ts=200 must survive a GC at 150.
        assert_eq!(csr.gc_property_versions(150), 0);
        let at_200 = csr.get_by_edge_id(eid, 200).unwrap();
        assert!(at_200
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
        // Past every active snapshot, history is reclaimed but the latest
        // value stays readable.
        assert!(csr.gc_property_versions(300) >= 1);
        let at_300 = csr.get_by_edge_id(eid, 300).unwrap();
        assert!(at_300
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(3.0))));
    }

    #[test]
    fn projected_read_returns_only_requested_columns() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(5);
        csr.insert_for_edge(
            eid,
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("label".to_string(), Value::String("a".into())),
            ],
            100,
        )
        .unwrap();

        let all = csr.get_projected_by_edge_id(eid, 100, None).unwrap();
        assert_eq!(all.len(), 2);

        let subset = csr
            .get_projected_by_edge_id(eid, 100, Some(&["label".to_string()]))
            .unwrap();
        assert_eq!(
            subset,
            vec![("label".to_string(), Some(Value::String("a".into())))]
        );

        let topology_only = csr.get_projected_by_edge_id(eid, 100, Some(&[])).unwrap();
        assert!(topology_only.is_empty());

        let unknown = csr
            .get_projected_by_edge_id(eid, 100, Some(&["missing".to_string()]))
            .unwrap();
        assert!(unknown.is_empty());

        assert!(csr.get_projected_by_edge_id(eid, 99, None).is_none());
        assert!(csr
            .get_projected_by_edge_id(eid, 99, Some(&["weight".to_string()]))
            .is_none());
    }

    #[test]
    fn dump_load_roundtrip() {
        let mut csr = CsrWithProperties::new(schema());
        let eid0 = EdgeId(10);
        let eid1 = EdgeId(11);

        csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        csr.mark_deleted(eid0, 150);

        let bytes = csr.dump();
        let mut loaded = CsrWithProperties::new(schema());
        loaded.load(&bytes).unwrap();

        assert_eq!(loaded.row_count(), 2);
        assert!(loaded
            .get_by_edge_id(eid1, 100)
            .unwrap()
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
        assert!(loaded.get_by_edge_id(eid0, 149).is_some());
        assert!(loaded.get_by_edge_id(eid0, 150).is_none());
    }

    #[test]
    fn truncated_payload_is_rejected() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(10);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let bytes = csr.dump();
        let mut loaded = CsrWithProperties::new(schema());
        assert!(loaded.load(&bytes[..bytes.len() / 2]).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut csr = CsrWithProperties::new(schema());
        csr.insert_for_edge(
            EdgeId(10),
            &[("weight".to_string(), Value::Double(1.0))],
            100,
        )
        .unwrap();
        let mut bytes = csr.dump();
        bytes.push(0xff);
        let mut loaded = CsrWithProperties::new(schema());
        assert!(loaded.load(&bytes).is_err());
    }

    fn typed_store() -> CsrWithProperties {
        CsrWithProperties::new(vec![
            PropertySchema::new("count".to_string(), 0, DataType::Int),
            PropertySchema::new("flag".to_string(), 1, DataType::Bool),
            PropertySchema::new("tag".to_string(), 2, DataType::String).nullable(true),
        ])
    }

    fn fill_typed_store(rows: i64) -> CsrWithProperties {
        let mut csr = typed_store();
        for i in 0..rows {
            csr.insert_for_edge(
                EdgeId(i as u64),
                &[
                    ("count".to_string(), Value::Int(i as i32)),
                    ("flag".to_string(), Value::Bool(i % 2 == 0)),
                    (
                        "tag".to_string(),
                        Value::String(format!("tag{}", i % 4).into()),
                    ),
                ],
                100,
            )
            .expect("typed insert should succeed");
        }
        csr
    }

    #[test]
    fn dump_collapses_version_history_to_latest() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(42);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        let bytes = csr.dump();
        let mut loaded = CsrWithProperties::new(schema());
        loaded.load(&bytes).unwrap();
        let collapsed = loaded
            .get_by_edge_id(eid, 150)
            .expect("row survives reload");
        assert!(collapsed
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    }

    #[test]
    fn dump_restores_recorded_encodings_with_current_values() {
        let mut csr = fill_typed_store(20);
        assert!(csr.auto_encode_properties() > 0);
        let before = csr.column_encoding_type("count");
        assert!(before.is_some_and(|enc| enc != EncodingType::None));
        let bytes = csr.dump();
        let mut loaded = typed_store();
        loaded.load(&bytes).unwrap();
        assert_eq!(loaded.column_encoding_type("count"), before);
        let got = loaded
            .get_by_edge_id(EdgeId(3), 200)
            .expect("row should read");
        assert!(got
            .iter()
            .any(|(k, v)| k == "count" && v == &Some(Value::Int(3))));
    }

    #[test]
    fn dump_persists_statistics_without_refresh() {
        let mut csr = fill_typed_store(10);
        csr.refresh_column_stats();
        let before = csr
            .column_stats_snapshot("count")
            .expect("stats should exist");
        assert!(before.null_count.is_some());
        let bytes = csr.dump();
        let mut loaded = typed_store();
        loaded.load(&bytes).unwrap();
        let after = loaded
            .column_stats_snapshot("count")
            .expect("stats should survive reload");
        assert_eq!(after.row_count, before.row_count);
        assert_eq!(after.null_count, before.null_count);
        assert_eq!(after.min_value, before.min_value);
        assert_eq!(after.max_value, before.max_value);
    }

    #[test]
    fn duplicate_column_identifiers_are_rejected() {
        let csr = fill_typed_store(2);
        let mut bytes = csr.dump();
        // Patch the second column header identifier to collide with the
        // first: name length plus name precedes the identifier.
        let mut cursor = 0usize;
        let read_u32 = |cursor: &mut usize| {
            let value = u32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
            *cursor += 4;
            value
        };
        let vis_len = read_u32(&mut cursor) as usize;
        cursor += vis_len * 9 + 4;
        let map_len = read_u32(&mut cursor) as usize;
        cursor += map_len * 12;
        let free_len = read_u32(&mut cursor) as usize;
        cursor += free_len * 4;
        let col_count = read_u32(&mut cursor);
        assert!(col_count >= 2);
        let first_name_len = read_u32(&mut cursor) as usize;
        cursor += first_name_len;
        let first_id = cursor;
        cursor += 4 + 1;
        let first_rows = read_u32(&mut cursor) as usize;
        // Cells hold typed values of unknown byte length here: skip them by
        // re-reading flags and lengths from the payload itself.
        for _ in 0..first_rows {
            let has = bytes[cursor];
            cursor += 1;
            if has == 1 {
                let vlen = read_u32(&mut cursor) as usize;
                cursor += vlen;
            }
        }
        let stats_flag = bytes[cursor];
        cursor += 1;
        if stats_flag == 1 {
            let stats_len = read_u32(&mut cursor) as usize;
            cursor += stats_len;
        }
        let second_name_len = read_u32(&mut cursor) as usize;
        cursor += second_name_len;
        let second_id = cursor;
        bytes.copy_within(first_id..first_id + 4, second_id);
        let mut loaded = typed_store();
        assert!(loaded.load(&bytes).is_err());
    }

    #[test]
    fn auto_encode_selects_expected_schemes() {
        let mut csr = fill_typed_store(20);
        assert_eq!(csr.auto_encode_properties(), 3);
        assert_eq!(
            csr.column_encoding_type("count"),
            Some(EncodingType::BitPacking)
        );
        assert_eq!(csr.column_encoding_type("flag"), Some(EncodingType::Rle));
        assert_eq!(
            csr.column_encoding_type("tag"),
            Some(EncodingType::Dictionary)
        );
    }

    #[test]
    fn auto_encode_preserves_values() {
        let mut csr = fill_typed_store(20);
        csr.auto_encode_properties();
        for i in 0..20 {
            let got = csr
                .get_by_edge_id(EdgeId(i as u64), 200)
                .expect("encoded row should stay readable");
            assert!(got
                .iter()
                .any(|(k, v)| k == "count" && v == &Some(Value::Int(i))));
            assert!(got
                .iter()
                .any(|(k, v)| k == "flag" && v == &Some(Value::Bool(i % 2 == 0))));
            let expected_tag = Value::String(format!("tag{}", i % 4).into());
            assert!(got
                .iter()
                .any(|(k, v)| k == "tag" && v == &Some(expected_tag.clone())));
        }
    }

    #[test]
    fn auto_encode_constant_column_uses_single_value_storage() {
        let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
            "level".to_string(),
            0,
            DataType::Int,
        )]);
        for i in 0..60 {
            csr.insert_for_edge(EdgeId(i), &[("level".to_string(), Value::Int(7))], 100)
                .expect("constant insert should succeed");
        }
        assert_eq!(csr.auto_encode_properties(), 1);
        assert_eq!(
            csr.column_encoding_type("level"),
            Some(EncodingType::Constant)
        );
        let got = csr.get_by_edge_id(EdgeId(3), 200).expect("row should read");
        assert!(got
            .iter()
            .any(|(k, v)| k == "level" && v == &Some(Value::Int(7))));
    }

    #[test]
    fn encode_rejects_unknown_column_and_skips_empty() {
        let mut csr = typed_store();
        assert_eq!(csr.auto_encode_properties(), 0);
        assert!(csr
            .apply_encoding_to_column("missing", EncodingType::Rle, 255)
            .is_err());
        assert_eq!(csr.column_encoding_type("missing"), None);
    }

    #[test]
    fn refresh_stats_feeds_snapshot() {
        let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
            "count".to_string(),
            0,
            DataType::Int,
        )
        .nullable(true)]);
        for i in 0..5 {
            csr.insert_for_edge(
                EdgeId(i),
                &[("count".to_string(), Value::Int((i as i32 + 1) * 10))],
                100,
            )
            .expect("stat insert should succeed");
        }
        csr.insert_for_edge(EdgeId(99), &[], 100)
            .expect("null insert should succeed");
        // Materialize the absent cell as an explicit null so the flush-time
        // statistics count it, matching the read path that serves it as null.
        csr.set_property_for_edge(EdgeId(99), "count", None, 150)
            .expect("null write should succeed");
        // Zone-map bounds are live before any refresh, but persisted counts
        // only exist after the flush-time refresh.
        let before = csr.column_stats_snapshot("count").expect("snapshot exists");
        assert_eq!(before.row_count, 6);
        assert_eq!(before.min_value, Some(Value::Int(10)));
        assert_eq!(before.max_value, Some(Value::Int(50)));
        assert_eq!(before.null_count, None);
        csr.refresh_column_stats();
        let after = csr.column_stats_snapshot("count").expect("snapshot exists");
        assert_eq!(after.row_count, 6);
        assert_eq!(after.min_value, Some(Value::Int(10)));
        assert_eq!(after.max_value, Some(Value::Int(50)));
        assert_eq!(after.null_count, Some(1));
        assert!(after.distinct_count.is_some());
        assert!(csr.column_stats_snapshot("missing").is_none());
    }

    #[test]
    fn truncated_payloads_are_rejected() {
        let mut csr = CsrWithProperties::new(schema());
        csr.insert_for_edge(
            EdgeId(10),
            &[("weight".to_string(), Value::Double(1.0))],
            100,
        )
        .unwrap();
        let bytes = csr.dump();
        assert!(!bytes.is_empty());
        for cut in [1, 5, 9, bytes.len() / 2, bytes.len() - 1] {
            let mut loaded = CsrWithProperties::new(schema());
            assert!(
                loaded.load(&bytes[..cut]).is_err(),
                "cut at {} must fail",
                cut
            );
        }
        let mut empty = CsrWithProperties::new(schema());
        assert!(empty.load(&[]).is_err());
    }

    #[test]
    fn release_never_admits_virgin_rows() {
        let mut csr = CsrWithProperties::new(schema());
        csr.release_row(0);
        csr.release_row(999);
        assert!(csr.edge_ids().next().is_none());
        let eid = EdgeId(11);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let row = csr.get_row_for_edge(eid).expect("row exists");
        csr.release_row(row);
        assert!(csr.get_row_for_edge(eid).is_none());
        // Releasing the same used row twice never duplicates free slots.
        csr.release_row(row);
    }

    #[test]
    fn stable_column_ids_survive_column_drop() {
        let mut csr = CsrWithProperties::new(vec![
            PropertySchema::new("a".to_string(), 0, DataType::Int),
            PropertySchema::new("b".to_string(), 1, DataType::Int),
            PropertySchema::new("c".to_string(), 2, DataType::Int),
        ]);
        let eid = EdgeId(1);
        csr.insert_for_edge(
            eid,
            &[
                ("a".to_string(), Value::Int(1)),
                ("b".to_string(), Value::Int(2)),
                ("c".to_string(), Value::Int(3)),
            ],
            100,
        )
        .unwrap();
        let id_c = csr.get_property_id("c").expect("c id exists");
        csr.remove_property("a").unwrap();
        // Stored undo parameters keyed by stable id still address column c.
        csr.set_property_by_id_for_edge(eid, id_c, Some(Value::Int(30)), 110)
            .expect("stable id must survive column drop");
        let got = csr.get_by_edge_id(eid, 110).expect("row readable");
        assert!(got
            .iter()
            .any(|(k, v)| k == "c" && v == &Some(Value::Int(30))));
        // Dropped columns stay unknown by name and by stale position.
        assert!(csr.get_property_id("a").is_none());
    }

    #[test]
    fn physical_projection_ignores_row_visibility() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(21);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.mark_deleted(eid, 150);
        assert!(csr.get_projected_by_edge_id(eid, 200, None).is_none());
        let physical = csr
            .get_projected_physical_by_edge_id(eid, 200, None)
            .expect("physical row survives");
        assert_eq!(physical.len(), 2);
    }
}
