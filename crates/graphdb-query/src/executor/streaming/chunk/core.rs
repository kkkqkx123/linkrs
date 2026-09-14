//! DataChunk core: struct definition, construction, and basic access

use super::schema::{ColumnInfo, Schema};
use super::typed::TypedColumn;
use crate::executor::base::MemoryReservation;
use crate::executor::streaming::runtime::ColumnarStats;
use crate::executor::streaming::slot::{SlotId, SlotLayout};
use graphdb_core::columnar::MaterializedBatch;
use graphdb_core::Value;
use std::sync::Arc;

/// A chunk of rows processed in streaming execution
#[derive(Debug)]
pub struct DataChunk {
    /// Row data with Value types
    pub rows: Vec<Vec<Value>>,
    /// Optional column-major representation for efficient columnar access.
    pub columns: Option<Vec<Vec<Value>>>,
    /// Optional typed column layout.
    pub typed_columns: Option<Vec<TypedColumn>>,
    /// Selection vector.
    pub selection: Option<Vec<usize>>,
    /// Symbolic row multiplicity (Ladybug `ResultSet::multiplicity` analogue).
    ///
    /// Transparent operators may bump this instead of physically duplicating
    /// rows. `1` means every visible row appears once. The expanded
    /// (flat) row count is `visible_count * multiplicity`.
    pub multiplicity: u64,
    /// Schema information (column names and types)
    pub schema: Arc<Schema>,
    /// Slot layout for slot-based value access.
    pub layout: Arc<SlotLayout>,
    /// Memory reservation for this chunk's data.
    pub memory_reservation: Option<MemoryReservation>,
    /// Query-level columnar fast-path counters (observability).
    pub columnar_stats: Option<Arc<ColumnarStats>>,
}

impl Clone for DataChunk {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            columns: self.columns.clone(),
            typed_columns: self.typed_columns.clone(),
            selection: self.selection.clone(),
            multiplicity: self.multiplicity,
            schema: self.schema.clone(),
            layout: Arc::clone(&self.layout),
            memory_reservation: None,
            columnar_stats: self.columnar_stats.clone(),
        }
    }
}

impl DataChunk {
    // ── Construction ──

    pub fn new(rows: Vec<Vec<Value>>, schema: Arc<Schema>) -> Self {
        let layout = Arc::new(SlotLayout::from_names(
            &schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
        ));
        Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            multiplicity: 1,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    pub fn with_memory_reservation(mut self, reservation: MemoryReservation) -> Self {
        self.memory_reservation = Some(reservation);
        self
    }

    pub fn with_columnar_stats(mut self, stats: Arc<ColumnarStats>) -> Self {
        self.columnar_stats = Some(stats);
        self
    }

    pub fn take_memory_reservation(&mut self) -> Option<MemoryReservation> {
        self.memory_reservation.take()
    }

    pub fn new_with_layout(rows: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
        Self::try_new_with_layout(rows, layout).expect("DataChunk row width mismatch")
    }

    pub fn try_new_with_layout(
        rows: Vec<Vec<Value>>,
        layout: Arc<SlotLayout>,
    ) -> Result<Self, graphdb_core::error::QueryError> {
        let row_width = rows.first().map(Vec::len).unwrap_or(0);
        if !layout.is_empty()
            && !rows.is_empty()
            && !rows.iter().all(|row| row.len() == layout.len())
        {
            return Err(graphdb_core::error::QueryError::execution(format!(
                "DataChunk::new_with_layout: row width {} does not match layout width {}",
                row_width,
                layout.len()
            )));
        }
        let columns: Vec<ColumnInfo> = layout
            .slots
            .iter()
            .map(|info| ColumnInfo {
                name: info.name.clone(),
                data_type: info
                    .data_type
                    .as_ref()
                    .map(|dt| dt.to_string().to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string()),
            })
            .collect();
        let schema = Arc::new(Schema::new(columns));
        Ok(Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            multiplicity: 1,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        })
    }

    pub fn from_rows(rows: Vec<Vec<Value>>) -> Self {
        Self::from_rows_with_col_names(rows, None)
    }

    pub fn from_rows_with_col_names(rows: Vec<Vec<Value>>, col_names: Option<Vec<String>>) -> Self {
        let schema = if rows.is_empty() {
            if let Some(names) = col_names {
                Arc::new(Schema::new(
                    names
                        .into_iter()
                        .map(|name| ColumnInfo {
                            name,
                            data_type: "unknown".to_string(),
                        })
                        .collect(),
                ))
            } else {
                Arc::new(Schema::empty())
            }
        } else {
            let col_count = rows[0].len();
            let columns = (0..col_count)
                .map(|i| {
                    let name = col_names
                        .as_ref()
                        .and_then(|names| names.get(i).cloned())
                        .unwrap_or_else(|| format!("col_{}", i));

                    let data_type = if let Some(row) = rows.first() {
                        if let Some(val) = row.get(i) {
                            match val {
                                Value::BigInt(_) => "bigint",
                                Value::Int(_) => "int",
                                Value::Double(_) => "double",
                                Value::Float(_) => "float",
                                Value::String(_) => "string",
                                Value::Bool(_) => "bool",
                                Value::Null(_) => "null",
                                _ => "unknown",
                            }
                        } else {
                            "unknown"
                        }
                    } else {
                        "unknown"
                    };

                    ColumnInfo {
                        name,
                        data_type: data_type.to_string(),
                    }
                })
                .collect();
            Arc::new(Schema::new(columns))
        };
        let layout = Arc::new(SlotLayout::from_names(
            &schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
        ));
        Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            multiplicity: 1,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    pub(crate) fn from_columns(columns: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
        // Compatibility transpose: row storage is authoritative and column
        // caches are derived. Production code must use `project_columns`;
        // this stays crate-private for row-major test fixtures only.
        let num_cols = columns.len();
        assert!(
            layout.is_empty() || num_cols == layout.len(),
            "DataChunk::from_columns: column count {} does not match layout width {}",
            num_cols,
            layout.len()
        );
        let num_rows = columns.first().map(|c| c.len()).unwrap_or(0);
        assert!(
            columns.iter().all(|c| c.len() == num_rows),
            "DataChunk::from_columns: column length mismatch"
        );

        let mut rows = vec![Vec::with_capacity(num_cols); num_rows];
        for col in columns.iter().take(num_cols) {
            for (row_idx, val) in col.iter().enumerate().take(num_rows) {
                rows[row_idx].push(val.clone());
            }
        }

        let schema = Arc::new(Schema::new(
            layout
                .slots
                .iter()
                .map(|info| ColumnInfo {
                    name: info.name.clone(),
                    data_type: info
                        .data_type
                        .as_ref()
                        .map(|dt| dt.to_string().to_lowercase())
                        .unwrap_or_else(|| "unknown".to_string()),
                })
                .collect(),
        ));

        Self {
            rows,
            columns: Some(columns),
            typed_columns: None,
            selection: None,
            multiplicity: 1,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    /// Columnar projection assembly: build an output chunk from evaluated
    /// output columns without a row intermediate.
    ///
    /// This is the designated constructor for the streaming `Project` fast
    /// path (passthrough / constant columns gathered or broadcast upstream).
    /// Row storage stays authoritative downstream; the transpose into rows
    /// is the single compatibility edge until the chunk itself is columnar.
    /// The typed rebuild here is unconditional by design: projected outputs
    /// are bounded by the input chunk size, unlike join outputs which honor
    /// the shared columnar gate.
    pub fn project_columns(columns: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
        let mut chunk = Self::from_columns(columns, layout);
        // Symmetric with the `Assign` rebuild: the projected chunk starts
        // row-major, so re-derive the typed layout here; otherwise every
        // non-trivial projection drops the columnar fast path downstream.
        chunk.build_typed_columns(true);
        chunk
    }

    /// Build a row chunk from a columnar batch slice.
    ///
    /// Coexistence API: the batch stays the operator state authority while
    /// the chunk carries the row view downstream. A transpose is still
    /// performed internally until the chunk itself becomes columnar.
    pub fn from_batch(batch: &MaterializedBatch, layout: Arc<SlotLayout>) -> Self {
        let rows = batch.to_rows();
        Self::try_new_with_layout(rows, layout).expect("DataChunk row width mismatch")
    }

    /// Build a row chunk from a row range of a columnar batch.
    pub fn slice_from_batch(
        batch: &MaterializedBatch,
        offset: usize,
        len: usize,
        layout: Arc<SlotLayout>,
    ) -> Self {
        Self::from_batch(&batch.slice_rows(offset, len), layout)
    }

    /// Convert this chunk back into a columnar batch (single transpose edge).
    pub fn to_batch(&self) -> MaterializedBatch {
        let mut batch = MaterializedBatch::new(self.num_columns(), 0);
        let names: Arc<[String]> = Arc::from(
            self.schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
        );
        batch.set_schema_names(names);
        for row in &self.rows {
            batch.append_row(row.clone());
        }
        batch
    }

    /// Memory estimate for the *visible expanded* rows
    /// (`visible_count * multiplicity`), used for pool/tracker accounting.
    pub fn estimated_bytes(&self) -> usize {
        let base = crate::executor::base::MemoryBudget::estimate_rows_memory(&self.rows);
        if self.rows.is_empty() || base == 0 {
            return 0;
        }
        let visible = self.visible_count();
        if visible == self.rows.len() && self.multiplicity <= 1 {
            return base;
        }
        let per_row = base.saturating_div(self.rows.len().max(1));
        (visible as u64)
            .saturating_mul(self.multiplicity)
            .saturating_mul(per_row as u64)
            .min(usize::MAX as u64) as usize
    }

    // ── Basic access ──

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn num_columns(&self) -> usize {
        self.schema.column_count()
    }

    pub fn col_names(&self) -> Vec<String> {
        self.schema.columns.iter().map(|c| c.name.clone()).collect()
    }

    pub fn col_name(&self, index: usize) -> Option<String> {
        self.schema.columns.get(index).map(|c| c.name.clone())
    }

    pub fn col_name_index(&self) -> std::collections::HashMap<String, usize> {
        self.schema
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.name.clone(), i))
            .collect()
    }

    pub fn get_layout(&self) -> Arc<SlotLayout> {
        Arc::clone(&self.layout)
    }

    pub fn get_by_slot(&self, row_idx: usize, slot: SlotId) -> Option<Value> {
        self.rows
            .get(row_idx)
            .and_then(|row| row.get(slot).cloned())
    }

    /// Compatibility column view over `Value`s. New expression code should
    /// prefer `typed_column` and only use this for join keys and fallback
    /// paths that require owned `Value` columns.
    pub fn get_column(&mut self, slot: SlotId) -> Option<Vec<Value>> {
        if slot >= self.layout.len() {
            return None;
        }
        if self.columns.is_none() && !self.rows.is_empty() {
            self.materialize_columns();
        }
        if let Some(ref columns) = self.columns {
            return columns.get(slot).cloned();
        }
        Some(self.rows.iter().map(|row| row[slot].clone()).collect())
    }

    pub fn column_ref(&self, slot: SlotId) -> Option<Vec<&Value>> {
        if slot >= self.layout.len() {
            return None;
        }
        Some(self.rows.iter().map(|row| &row[slot]).collect())
    }

    pub fn get_typed_by_slot(&self, row_idx: usize, slot: SlotId) -> Option<Value> {
        if let Some(ref typed) = self.typed_columns {
            if let Some(col) = typed.get(slot) {
                return col.value_at(row_idx);
            }
        }
        self.get_by_slot(row_idx, slot)
    }

    pub fn row_at(&self, i: usize) -> &[Value] {
        &self.rows[i]
    }

    /// Take the physical rows out for buffer reuse, resetting selection,
    /// multiplicity, and derived column caches.
    ///
    /// Unlike [`Clone`](Self::clone) (documented deep copy), this moves the
    /// allocation out so the caller can return the drained buffers to
    /// [`super::pool::RowBufferPool`] via `release_rows` instead of dropping
    /// them. Prefer move-first construction plus this pool over cloning.
    pub fn take_rows_for_reuse(&mut self) -> Vec<Vec<Value>> {
        self.selection = None;
        self.columns = None;
        if self.typed_columns.take().is_some() {
            // The typed layout is dropped without a downstream consumer; count
            // it so a high build cost with no reader becomes visible.
            if let Some(stats) = &self.columnar_stats {
                stats.record_wasted_build();
            }
        }
        self.multiplicity = 1;
        std::mem::take(&mut self.rows)
    }

    // ── Typed column layout ──

    /// Build the typed column layout for this chunk.
    ///
    /// `use_columnar` carries the adaptive [`ColumnarPolicy`] decision from
    /// the producing operator: when the learned hit rate falls below the
    /// threshold the chunk stays row-based. The typed layout is otherwise
    /// always attempted (there is no global off switch).
    /// Returns the number of extra typed bytes allocated.
    pub fn build_typed_columns(&mut self, use_columnar: bool) -> usize {
        if !use_columnar || self.typed_columns.is_some() {
            return 0;
        }
        // Global memory pressure: skip building new acceleration caches and
        // stay on the plain row path (see `memory_watermark` degradation
        // contract).
        if !graphdb_storage::memory_watermark::pressure().allows_columnar() {
            return 0;
        }
        let num_cols = self.num_columns();
        if self.rows.is_empty() || num_cols == 0 {
            return 0;
        }
        let num_rows = self.rows.len();
        let mut typed = Vec::with_capacity(num_cols);
        let mut extra_bytes = 0usize;
        for col_idx in 0..num_cols {
            let first = &self.rows[0][col_idx];
            let representative = if matches!(first, Value::Null(_)) {
                // A leading NULL cannot reveal the column kind; probe the
                // first non-NULL value so NULL-leading homogeneous columns
                // stay on the typed path (all-NULL columns fall back).
                super::kind::first_non_null(self.rows.iter().map(|row| &row[col_idx]))
                    .unwrap_or(first)
            } else {
                first
            };
            let Some(kind) = super::kind::value_to_kind(representative) else {
                // A non-scalar value cannot reveal the column kind: fall back
                // so the per-row path keeps the exact `Value` semantics.
                typed.push(TypedColumn::Fallback(
                    self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                ));
                continue;
            };
            let mut ok = true;
            let mut has_null = false;
            let mut bitmap = vec![0u64; num_rows.div_ceil(64)];
            let mut builder = super::kind::TypedColumnBuilder::with_capacity(kind, num_rows);
            for (i, row) in self.rows.iter().enumerate() {
                match builder.push_value(&row[col_idx]) {
                    super::kind::PushOutcome::Value => bitmap[i / 64] |= 1u64 << (i % 64),
                    super::kind::PushOutcome::Null => has_null = true,
                    super::kind::PushOutcome::Mismatch => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                extra_bytes += builder.estimated_bytes();
                extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                typed.push(builder.finish(has_null, bitmap));
            } else {
                typed.push(TypedColumn::Fallback(
                    self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                ));
            }
        }
        self.typed_columns = Some(typed);
        extra_bytes
    }

    pub fn typed_column(&self, slot: SlotId) -> Option<&TypedColumn> {
        self.typed_columns.as_ref().and_then(|cols| cols.get(slot))
    }

    // ── Column materialization ──

    pub fn materialize_columns(&mut self) {
        // Compatibility transpose cache: prefer `typed_columns` when valid,
        // else derive a cloned column view from rows. Not a storage format.
        if self.columns.is_some() {
            return;
        }
        if let Some(ref typed) = self.typed_columns {
            if typed.len() == self.num_columns() && !self.rows.is_empty() {
                self.columns = Some(typed.iter().map(TypedColumn::to_values).collect());
                return;
            }
        }
        let num_cols = self.num_columns();
        if self.rows.is_empty() || num_cols == 0 {
            self.columns = Some(Vec::new());
            return;
        }
        let num_rows = self.rows.len();
        let mut columns = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let mut col = Vec::with_capacity(num_rows);
            for row in &self.rows {
                col.push(row[col_idx].clone());
            }
            columns.push(col);
        }
        self.columns = Some(columns);
    }

    // ── Columnar stats helpers ──

    pub(super) fn count_columnar(&self, hit: bool) {
        if let Some(stats) = &self.columnar_stats {
            if hit {
                stats.record_hit();
            } else {
                stats.record_miss();
            }
        }
    }

    pub(crate) fn count_typed_hit(&self) {
        if let Some(stats) = &self.columnar_stats {
            stats.record_typed_hit();
        }
    }

    /// record an evaluation served by the selection-aware visible-row
    /// fast path (selection consumed in place, no materialization).
    pub(super) fn count_selection_pushed(&self) {
        if let Some(stats) = &self.columnar_stats {
            stats.record_selection_pushed();
        }
    }
}
