use graphdb_core::types::DataType;

/// Raw decoded values for one property column, in column-major order.
///
/// Fixed-size scalar columns (BigInt/Double/Int) are returned as dense typed
/// vectors plus a validity bitmap (`valid[i] == 1` means the value is
/// present, not null).  Everything else (strings, mixed, other types) falls
/// back to per-row decoded `Option<Value>`.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnValues {
    I64 { values: Vec<i64>, valid: Vec<u8> },
    F64 { values: Vec<f64>, valid: Vec<u8> },
    I32 { values: Vec<i32>, valid: Vec<u8> },
    General(Vec<Option<graphdb_core::Value>>),
}

impl ColumnValues {
    /// Number of rows in this column.
    pub fn len(&self) -> usize {
        match self {
            ColumnValues::I64 { values, .. } => values.len(),
            ColumnValues::F64 { values, .. } => values.len(),
            ColumnValues::I32 { values, .. } => values.len(),
            ColumnValues::General(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The decoded value at row `idx` (None for null / missing).
    pub fn value_at(&self, idx: usize) -> Option<graphdb_core::Value> {
        match self {
            ColumnValues::I64 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::BigInt(v))
                } else {
                    None
                }
            }
            ColumnValues::F64 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::Double(v))
                } else {
                    None
                }
            }
            ColumnValues::I32 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::Int(v))
                } else {
                    None
                }
            }
            ColumnValues::General(values) => values.get(idx).cloned().flatten(),
        }
    }

    /// Append another column's rows (same kind). Kind mismatches are resolved
    /// by degrading both sides to `General`, except when the target is an
    /// empty `General` — then the source's typed kind is adopted so the
    /// first table's decode stays typed.
    pub fn append(&mut self, other: ColumnValues) {
        // Adopt the source's typed kind when the target is an empty `General`
        // column so the first decoded run keeps its typed layout.
        if matches!(self, ColumnValues::General(values) if values.is_empty()) {
            *self = other;
            return;
        }
        match (self, other) {
            (
                ColumnValues::I64 { values, valid },
                ColumnValues::I64 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::F64 { values, valid },
                ColumnValues::F64 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::I32 { values, valid },
                ColumnValues::I32 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (ColumnValues::General(values), ColumnValues::General(values2)) => {
                values.extend(values2);
            }
            (self_col, other) => {
                let mut general = self_col.to_general();
                general.extend(other.to_general());
                *self_col = ColumnValues::General(general);
            }
        }
    }

    /// Append `n` null rows (used when merging columns across tables that
    /// lack a column).
    pub fn append_nulls(&mut self, n: usize) {
        match self {
            ColumnValues::I64 { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::F64 { values, valid } => {
                values.resize(values.len() + n, 0.0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::I32 { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::General(values) => {
                values.resize(values.len() + n, None);
            }
        }
    }

    /// Truncate the column to the first `n` rows.
    pub fn truncate(&mut self, n: usize) {
        match self {
            ColumnValues::I64 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::F64 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::I32 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::General(values) => values.truncate(n),
        }
    }

    /// Compress the column to the rows where `keep[i]` is true, in order.
    pub fn compact(&mut self, keep: &[bool]) {
        match self {
            ColumnValues::I64 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::F64 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::I32 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::General(values) => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i].take();
                        write += 1;
                    }
                }
                values.truncate(write);
            }
        }
    }

    /// Convert to a `General` per-row `Option<Value>` column.
    pub fn to_general(&self) -> Vec<Option<graphdb_core::Value>> {
        (0..self.len()).map(|i| self.value_at(i)).collect()
    }

    /// Scatter this column's rows into `target` at the given output
    /// positions (used when merging per-shard decodes back into input order).
    /// `positions[i]` is `(out_idx, local_id)`; the local id is ignored here.
    /// `target` must be a `General` column pre-sized to the merged row count.
    pub fn scatter(&self, target: &mut ColumnValues, positions: &[(usize, u32)]) {
        let ColumnValues::General(target_rows) = target else {
            return;
        };
        for (i, &(out_idx, _)) in positions.iter().enumerate() {
            if let Some(value) = self.value_at(i) {
                target_rows[out_idx] = Some(value);
            }
        }
    }

    /// Convert a `General` per-row column into a typed column when every value
    /// matches the column's declared scalar kind (or is null).  Returns `None`
    /// when the declared type does not map to a typed kind or values disagree.
    pub fn from_general_with_type(
        values: Vec<Option<graphdb_core::Value>>,
        data_type: &DataType,
    ) -> Option<ColumnValues> {
        match data_type {
            DataType::BigInt => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::BigInt(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::I64 { values: vs, valid })
            }
            DataType::Double => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::Double(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0.0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::F64 { values: vs, valid })
            }
            DataType::Int => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::Int(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::I32 { values: vs, valid })
            }
            _ => None,
        }
    }

    /// Whether every row is non-null (so the typed fast path can be used
    /// without a validity bitmap).
    pub fn all_valid(&self) -> bool {
        match self {
            ColumnValues::I64 { valid, .. }
            | ColumnValues::F64 { valid, .. }
            | ColumnValues::I32 { valid, .. } => valid.iter().all(|&v| v == 1),
            ColumnValues::General(values) => values.iter().all(|v| v.is_some()),
        }
    }
}

/// One property column of a column-major vertex batch.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyColumn {
    pub name: String,
    pub data_type: DataType,
    pub values: ColumnValues,
}

/// A column-major vertex batch produced by `VertexCursor::next_column_batch`.
///
/// Rows are implicit: every column (and `vids`/`internal_ids`) has the same
/// length.  `columns` holds one entry per requested property in projection
/// order; when the scan requests a full-row decode (empty projection) it
/// holds every column of the scanned table(s).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexColumnBatch {
    pub vids: Vec<graphdb_core::types::VertexId>,
    pub internal_ids: Vec<i64>,
    /// Tag (label) name per row (batches may span tables).
    pub tag_names: Vec<String>,
    pub columns: Vec<PropertyColumn>,
}

impl VertexColumnBatch {
    pub fn empty() -> Self {
        Self {
            vids: Vec::new(),
            internal_ids: Vec::new(),
            tag_names: Vec::new(),
            columns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.vids.len()
    }
}

/// A column-major edge batch produced by `EdgeCursor::next_column_batch`.
///
/// Rows are implicit: every column (and `srcs`/`dsts`/`edge_types`/
/// `rankings`) has the same length.  `columns` holds one entry per requested
/// property in projection order.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeColumnBatch {
    pub srcs: Vec<graphdb_core::types::VertexId>,
    pub dsts: Vec<graphdb_core::types::VertexId>,
    pub edge_types: Vec<String>,
    pub rankings: Vec<i64>,
    pub columns: Vec<PropertyColumn>,
}

impl EdgeColumnBatch {
    pub fn empty() -> Self {
        Self {
            srcs: Vec::new(),
            dsts: Vec::new(),
            edge_types: Vec::new(),
            rankings: Vec::new(),
            columns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.srcs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.srcs.len()
    }
}
