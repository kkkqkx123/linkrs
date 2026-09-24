use graphdb_core::types::DataType;

/// Raw decoded values for one property column, in column-major order.
///
/// Fixed-size numeric and boolean columns (Bool/SmallInt/Int/BigInt/Float/Double)
/// are returned as dense typed vectors plus a validity bitmap (`valid[i] == 1`
/// means the value is present, not null). Date/Time/DateTime/Uuid columns and
/// everything else (strings, mixed, other types) fall back to per-row decoded
/// `Option<Value>`: extending the typed layout there would require matching
/// executor support, while the fallback preserves mixed-type reads without
/// changing results. A column whose rows do not share one numeric kind also
/// degrades to the fallback so no value is silently dropped.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnValues {
    I64 { values: Vec<i64>, valid: Vec<u8> },
    F64 { values: Vec<f64>, valid: Vec<u8> },
    I32 { values: Vec<i32>, valid: Vec<u8> },
    Bool { values: Vec<u8>, valid: Vec<u8> },
    I16 { values: Vec<i16>, valid: Vec<u8> },
    F32 { values: Vec<f32>, valid: Vec<u8> },
    General(Vec<Option<graphdb_core::Value>>),
}

impl ColumnValues {
    /// Number of rows in this column.
    pub fn len(&self) -> usize {
        match self {
            ColumnValues::I64 { values, .. } => values.len(),
            ColumnValues::F64 { values, .. } => values.len(),
            ColumnValues::I32 { values, .. } => values.len(),
            ColumnValues::Bool { values, .. } => values.len(),
            ColumnValues::I16 { values, .. } => values.len(),
            ColumnValues::F32 { values, .. } => values.len(),
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
            ColumnValues::Bool { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::Bool(v != 0))
                } else {
                    None
                }
            }
            ColumnValues::I16 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::SmallInt(v))
                } else {
                    None
                }
            }
            ColumnValues::F32 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| graphdb_core::Value::Float(v))
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
            (
                ColumnValues::Bool { values, valid },
                ColumnValues::Bool {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::I16 { values, valid },
                ColumnValues::I16 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::F32 { values, valid },
                ColumnValues::F32 {
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
            ColumnValues::Bool { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::I16 { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::F32 { values, valid } => {
                values.resize(values.len() + n, 0.0);
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
            ColumnValues::Bool { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::I16 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::F32 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::General(values) => values.truncate(n),
        }
    }

    /// Compress the column to the rows where `keep[i]` is true, in order.
    pub fn compact(&mut self, keep: &[bool]) {
        let selection: Vec<usize> = keep
            .iter()
            .enumerate()
            .filter_map(|(i, &k)| if k { Some(i) } else { None })
            .collect();
        self.select(&selection);
    }

    /// Compress the column to the rows at `selection` indices, in order.
    ///
    /// Selection-vector form of [`Self::compact`]: filtering produces an
    /// index sequence instead of a boolean mask, and surviving rows are
    /// gathered directly without decoding skipped rows again. Kind-specific
    /// gathering keeps typed layouts typed; callers keep the same selection
    /// for every column of the batch.
    pub fn select(&mut self, selection: &[usize]) {
        match self {
            ColumnValues::I64 { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::F64 { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::I32 { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::Bool { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::I16 { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::F32 { values, valid } => {
                let mut next_values = Vec::with_capacity(selection.len());
                let mut next_valid = Vec::with_capacity(selection.len());
                for &i in selection {
                    next_values.push(values[i]);
                    next_valid.push(valid[i]);
                }
                *values = next_values;
                *valid = next_valid;
            }
            ColumnValues::General(values) => {
                let mut next = Vec::with_capacity(selection.len());
                for &i in selection {
                    next.push(values[i].take());
                }
                *values = next;
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
    /// Same-kind typed targets are written directly; a kind mismatch degrades
    /// the target to `General` so no decoded value is lost.
    pub fn scatter(&self, target: &mut ColumnValues, positions: &[(usize, u32)]) {
        for (i, &(out_idx, _)) in positions.iter().enumerate() {
            let value = self.value_at(i);
            if !target.set_value_at(out_idx, value.clone()) {
                let mut general = target.to_general();
                if out_idx < general.len() {
                    general[out_idx] = value;
                }
                *target = ColumnValues::General(general);
            }
        }
    }

    /// Write one decoded value at `idx`. Returns false when the value's kind
    /// does not match a typed target (the caller degrades to `General`).
    fn set_value_at(&mut self, idx: usize, value: Option<graphdb_core::Value>) -> bool {
        match self {
            ColumnValues::I64 { values, valid } => match value {
                Some(graphdb_core::Value::BigInt(v)) => {
                    if idx < values.len() {
                        values[idx] = v;
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::F64 { values, valid } => match value {
                Some(graphdb_core::Value::Double(v)) => {
                    if idx < values.len() {
                        values[idx] = v;
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::I32 { values, valid } => match value {
                Some(graphdb_core::Value::Int(v)) => {
                    if idx < values.len() {
                        values[idx] = v;
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::Bool { values, valid } => match value {
                Some(graphdb_core::Value::Bool(v)) => {
                    if idx < values.len() {
                        values[idx] = u8::from(v);
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::I16 { values, valid } => match value {
                Some(graphdb_core::Value::SmallInt(v)) => {
                    if idx < values.len() {
                        values[idx] = v;
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::F32 { values, valid } => match value {
                Some(graphdb_core::Value::Float(v)) => {
                    if idx < values.len() {
                        values[idx] = v;
                        valid[idx] = 1;
                        true
                    } else {
                        false
                    }
                }
                None => {
                    if idx < valid.len() {
                        valid[idx] = 0;
                        true
                    } else {
                        false
                    }
                }
                Some(_) => false,
            },
            ColumnValues::General(target_rows) => {
                if idx < target_rows.len() {
                    target_rows[idx] = value;
                    true
                } else {
                    false
                }
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
            DataType::Bool => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::Bool(v)) => {
                            vs.push(u8::from(v));
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::Bool { values: vs, valid })
            }
            DataType::SmallInt => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::SmallInt(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::I16 { values: vs, valid })
            }
            DataType::Float => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(graphdb_core::Value::Float(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0.0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::F32 { values: vs, valid })
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
            | ColumnValues::I32 { valid, .. }
            | ColumnValues::Bool { valid, .. }
            | ColumnValues::I16 { valid, .. }
            | ColumnValues::F32 { valid, .. } => valid.iter().all(|&v| v == 1),
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

    /// Gather the batch down to `selection` indices, in order.
    pub fn select(&mut self, selection: &[usize]) {
        let gather_ids = selection.iter().map(|&i| self.vids[i]).collect::<Vec<_>>();
        let gather_internal = selection
            .iter()
            .map(|&i| self.internal_ids[i])
            .collect::<Vec<_>>();
        let gather_tags = selection
            .iter()
            .map(|&i| self.tag_names[i].clone())
            .collect::<Vec<_>>();
        self.vids = gather_ids;
        self.internal_ids = gather_internal;
        self.tag_names = gather_tags;
        for column in &mut self.columns {
            column.values.select(selection);
        }
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

    /// Gather the batch down to `selection` indices, in order.
    pub fn select(&mut self, selection: &[usize]) {
        let gather_srcs = selection.iter().map(|&i| self.srcs[i]).collect::<Vec<_>>();
        let gather_dsts = selection.iter().map(|&i| self.dsts[i]).collect::<Vec<_>>();
        let gather_types = selection
            .iter()
            .map(|&i| self.edge_types[i].clone())
            .collect::<Vec<_>>();
        let gather_ranks = selection
            .iter()
            .map(|&i| self.rankings[i])
            .collect::<Vec<_>>();
        self.srcs = gather_srcs;
        self.dsts = gather_dsts;
        self.edge_types = gather_types;
        self.rankings = gather_ranks;
        for column in &mut self.columns {
            column.values.select(selection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_typed_kinds_roundtrip_values() {
        let b = ColumnValues::Bool {
            values: vec![1, 0, 1],
            valid: vec![1, 1, 0],
        };
        assert_eq!(b.value_at(0), Some(graphdb_core::Value::Bool(true)));
        assert_eq!(b.value_at(1), Some(graphdb_core::Value::Bool(false)));
        assert_eq!(b.value_at(2), None);

        let mut with_null = b.clone();
        with_null.set_value_at(0, None);
        assert_eq!(with_null.value_at(0), None);
        assert!(!with_null.all_valid());

        let s = ColumnValues::I16 {
            values: vec![7, -3],
            valid: vec![1, 1],
        };
        assert_eq!(s.value_at(0), Some(graphdb_core::Value::SmallInt(7)));
        assert_eq!(s.value_at(1), Some(graphdb_core::Value::SmallInt(-3)));

        let f = ColumnValues::F32 {
            values: vec![1.5, 0.0],
            valid: vec![1, 0],
        };
        assert_eq!(f.value_at(0), Some(graphdb_core::Value::Float(1.5)));
        assert_eq!(f.value_at(1), None);
    }

    #[test]
    fn from_general_with_type_covers_new_kinds() {
        let bools = vec![
            Some(graphdb_core::Value::Bool(true)),
            None,
            Some(graphdb_core::Value::Bool(false)),
        ];
        let typed = ColumnValues::from_general_with_type(bools, &DataType::Bool).unwrap();
        assert!(matches!(typed, ColumnValues::Bool { .. }));
        assert_eq!(typed.len(), 3);

        let smalls = vec![Some(graphdb_core::Value::SmallInt(2)), None];
        let typed = ColumnValues::from_general_with_type(smalls, &DataType::SmallInt).unwrap();
        assert!(matches!(typed, ColumnValues::I16 { .. }));

        let floats = vec![Some(graphdb_core::Value::Float(2.5)), None];
        let typed = ColumnValues::from_general_with_type(floats, &DataType::Float).unwrap();
        assert!(matches!(typed, ColumnValues::F32 { .. }));

        let mixed = vec![Some(graphdb_core::Value::Int(1))];
        assert!(ColumnValues::from_general_with_type(mixed, &DataType::Bool).is_none());
    }

    #[test]
    fn scatter_into_typed_target_writes_directly() {
        let src = ColumnValues::I16 {
            values: vec![9, 8],
            valid: vec![1, 1],
        };
        let mut target = ColumnValues::I16 {
            values: vec![0; 4],
            valid: vec![0; 4],
        };
        src.scatter(&mut target, &[(1, 0), (3, 0)]);
        assert_eq!(target.value_at(0), None);
        assert_eq!(target.value_at(1), Some(graphdb_core::Value::SmallInt(9)));
        assert_eq!(target.value_at(2), None);
        assert_eq!(target.value_at(3), Some(graphdb_core::Value::SmallInt(8)));
    }

    #[test]
    fn scatter_kind_mismatch_degrades_to_general() {
        let src = ColumnValues::I16 {
            values: vec![9],
            valid: vec![1],
        };
        let mut target = ColumnValues::I32 {
            values: vec![0; 2],
            valid: vec![0; 2],
        };
        src.scatter(&mut target, &[(0, 0)]);
        assert!(matches!(target, ColumnValues::General(_)));
        assert_eq!(target.value_at(0), Some(graphdb_core::Value::SmallInt(9)));
        assert_eq!(target.value_at(1), None);
    }

    #[test]
    fn append_and_compact_cover_new_kinds() {
        let mut a = ColumnValues::Bool {
            values: vec![1],
            valid: vec![1],
        };
        a.append(ColumnValues::Bool {
            values: vec![0],
            valid: vec![0],
        });
        assert_eq!(a.len(), 2);
        assert_eq!(a.value_at(1), None);

        let mut f = ColumnValues::F32 {
            values: vec![1.0, 2.0, 3.0],
            valid: vec![1, 1, 1],
        };
        f.compact(&[true, false, true]);
        assert_eq!(f.len(), 2);
        assert_eq!(f.value_at(1), Some(graphdb_core::Value::Float(3.0)));

        let mut s = ColumnValues::I16 {
            values: vec![1],
            valid: vec![1],
        };
        s.append_nulls(2);
        assert_eq!(s.len(), 3);
        assert_eq!(s.value_at(2), None);
        s.truncate(1);
        assert_eq!(s.len(), 1);
    }
}
