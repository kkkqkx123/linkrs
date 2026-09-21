use super::CsrWithProperties;
use crate::edge::property_schema::PropertySchema;
use crate::vertex::column::Column;
use graphdb_core::{DataType, StorageError, StorageResult, Value};
use std::collections::{HashMap, HashSet};

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
            edge_map_segments: Vec::new(),
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

    pub(crate) fn reject_inline(&self) -> StorageResult<()> {
        if self.inline {
            return Err(StorageError::invalid_operation(
                "columnar property access on an inline-form table".to_string(),
            ));
        }
        Ok(())
    }

    /// Rebuild the column position indexes after a schema mutation.
    pub(crate) fn rebuild_schema_indexes(&mut self) {
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
}
