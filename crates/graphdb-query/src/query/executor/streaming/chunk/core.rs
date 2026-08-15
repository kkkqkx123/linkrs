//! DataChunk core: struct definition, construction, and basic access

use super::schema::{ColumnInfo, Schema};
use super::typed::{TypedColumn, TypedKind};
use super::view::ChunkView;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::base::MemoryReservation;
use crate::query::executor::streaming::runtime::ColumnarStats;
use crate::query::executor::streaming::slot::{SlotId, SlotLayout};
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
    ) -> Result<Self, crate::core::error::QueryError> {
        let row_width = rows.first().map(Vec::len).unwrap_or(0);
        if !layout.is_empty()
            && !rows.is_empty()
            && !rows.iter().all(|row| row.len() == layout.len())
        {
            return Err(crate::core::error::QueryError::execution(format!(
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
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    pub fn from_columns(columns: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
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
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    pub fn with_columns(mut self, columns: Vec<Vec<Value>>) -> Self {
        assert_eq!(columns.len(), self.num_columns(), "column count mismatch");
        if !self.rows.is_empty() {
            for col in &columns {
                assert_eq!(col.len(), self.len(), "column length mismatch");
            }
        }
        self.columns = Some(columns);
        self
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

    pub fn view(&self) -> ChunkView<'_> {
        ChunkView { rows: &self.rows }
    }

    // ── Typed column layout ──

    /// Build the typed column layout for this chunk.
    ///
    /// `use_columnar` carries the adaptive [`ColumnarPolicy`] decision from
    /// the producing operator: when the learned hit rate falls below the
    /// threshold (or the global switch is off), the chunk stays row-based.
    /// Returns the number of extra typed bytes allocated.
    pub fn build_typed_columns(&mut self, use_columnar: bool) -> usize {
        if !use_columnar || !super::typed_columns_enabled() || self.typed_columns.is_some() {
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
            let kind = match first {
                Value::BigInt(_) => Some(TypedKind::I64),
                Value::Double(_) => Some(TypedKind::F64),
                Value::Int(_) => Some(TypedKind::I32),
                Value::Bool(_) => Some(TypedKind::Bool),
                Value::Date(_) => Some(TypedKind::Date),
                Value::String(_) => Some(TypedKind::Utf8),
                // A leading NULL cannot reveal the column kind: fall back so
                // the per-row path keeps the exact `Value` semantics.
                Value::Null(_) => None,
                _ => None,
            };
            let Some(kind) = kind else {
                typed.push(TypedColumn::Fallback(
                    self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                ));
                continue;
            };
            let mut ok = true;
            let mut has_null = false;
            let mut bitmap = vec![0u64; num_rows.div_ceil(64)];
            let mut mark_valid = |i: usize| {
                bitmap[i / 64] |= 1u64 << (i % 64);
            };
            let column = match kind {
                TypedKind::I64 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::BigInt(v) => {
                                buf.push(v);
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(0);
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<i64>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableI64(buf, bitmap)
                        } else {
                            TypedColumn::I64(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::F64 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::Double(v) => {
                                buf.push(v);
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(0.0);
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<f64>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableF64(buf, bitmap)
                        } else {
                            TypedColumn::F64(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::I32 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::Int(v) => {
                                buf.push(v);
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(0);
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<i32>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableI32(buf, bitmap)
                        } else {
                            TypedColumn::I32(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::Bool => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::Bool(v) => {
                                buf.push(v);
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(false);
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<bool>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableBool(buf, bitmap)
                        } else {
                            TypedColumn::Bool(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::Date => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::Date(ref v) => {
                                buf.push(v.to_days());
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(0);
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<i64>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableDate(buf, bitmap)
                        } else {
                            TypedColumn::Date(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::Utf8 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for (i, row) in self.rows.iter().enumerate() {
                        match row[col_idx] {
                            Value::String(ref v) => {
                                buf.push(Arc::from(v.as_str()));
                                mark_valid(i);
                            }
                            Value::Null(NullType::Null) => {
                                has_null = true;
                                buf.push(Arc::from(""));
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf
                            .iter()
                            .map(|s: &Arc<str>| s.len() + std::mem::size_of::<Arc<str>>())
                            .sum::<usize>();
                        extra_bytes += bitmap.capacity() * std::mem::size_of::<u64>();
                        if has_null {
                            TypedColumn::NullableUtf8(buf, bitmap)
                        } else {
                            TypedColumn::Utf8(buf)
                        }
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
            };
            typed.push(column);
        }
        self.typed_columns = Some(typed);
        extra_bytes
    }

    pub fn typed_column(&self, slot: SlotId) -> Option<&TypedColumn> {
        self.typed_columns.as_ref().and_then(|cols| cols.get(slot))
    }

    // ── Column materialization ──

    pub fn materialize_columns(&mut self) {
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

    pub fn get_or_materialize_columns(&mut self) -> &[Vec<Value>] {
        self.materialize_columns();
        self.columns.as_ref().unwrap()
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

    pub(super) fn count_typed_hit(&self) {
        if let Some(stats) = &self.columnar_stats {
            stats.record_typed_hit();
        }
    }

    /// P2: record an evaluation served by the selection-aware visible-row
    /// fast path (selection consumed in place, no materialization).
    pub(super) fn count_selection_pushed(&self) {
        if let Some(stats) = &self.columnar_stats {
            stats.record_selection_pushed();
        }
    }
}
