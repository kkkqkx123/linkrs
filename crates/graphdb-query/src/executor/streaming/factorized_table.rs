use std::collections::HashMap;

use graphdb_core::Value;

use crate::planning::plan::factorization::FGroupPos;

/// Physical column schema for FactorizedTable.
///
/// `is_unflat = true` means the tuple stores an `OverflowValue` pointer,
/// otherwise the value is stored inline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSchema {
    pub is_unflat: bool,
    pub group_id: FGroupPos,
    pub num_bytes: u32,
    pub may_contain_nulls: bool,
}

impl ColumnSchema {
    pub fn new(is_unflat: bool, group_id: FGroupPos, num_bytes: u32) -> Self {
        Self {
            is_unflat,
            group_id,
            num_bytes,
            may_contain_nulls: false,
        }
    }

    pub fn is_flat(&self) -> bool {
        !self.is_unflat
    }

    pub fn has_no_null_guarantee(&self) -> bool {
        !self.may_contain_nulls
    }

    pub fn set_may_contain_nulls(&mut self) {
        self.may_contain_nulls = true;
    }
}

/// Physical schema for FactorizedTable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FactorizedTableSchema {
    pub columns: Vec<ColumnSchema>,
    pub num_bytes_for_data_per_tuple: u32,
    pub num_bytes_for_null_map_per_tuple: u32,
    pub num_bytes_per_tuple: u32,
    pub col_offsets: Vec<u32>,
}

impl FactorizedTableSchema {
    pub fn new(columns: Vec<ColumnSchema>) -> Self {
        let mut schema = Self {
            columns,
            ..Default::default()
        };
        schema.recompute_layout();
        schema
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    pub fn num_flat_columns(&self) -> usize {
        self.columns.iter().filter(|c| c.is_flat()).count()
    }

    pub fn num_unflat_columns(&self) -> usize {
        self.columns.iter().filter(|c| c.is_unflat).count()
    }

    pub fn get_column(&self, idx: usize) -> Option<&ColumnSchema> {
        self.columns.get(idx)
    }

    pub fn get_column_mut(&mut self, idx: usize) -> Option<&mut ColumnSchema> {
        self.columns.get_mut(idx)
    }

    pub fn col_offset(&self, idx: usize) -> u32 {
        self.col_offsets[idx]
    }

    pub fn null_map_offset(&self) -> u32 {
        self.num_bytes_for_data_per_tuple
    }

    pub fn append_column(&mut self, col: ColumnSchema) {
        self.columns.push(col);
        self.recompute_layout();
    }

    pub fn set_may_contain_nulls(&mut self, idx: usize) {
        if let Some(c) = self.columns.get_mut(idx) {
            c.set_may_contain_nulls();
        }
    }

    fn recompute_layout(&mut self) {
        let mut offset = 0u32;
        self.col_offsets.clear();
        for col in &self.columns {
            self.col_offsets.push(offset);
            offset += col.num_bytes;
        }
        self.num_bytes_for_data_per_tuple = offset;
        // Null bitmap: one bit per column, rounded up to bytes.
        let bits = self.columns.len();
        self.num_bytes_for_null_map_per_tuple = ((bits + 7) / 8) as u32;
        self.num_bytes_per_tuple =
            self.num_bytes_for_data_per_tuple + self.num_bytes_for_null_map_per_tuple;
    }
}

/// Overflow value for unflat columns: holds a pointer to overflow block.
///
/// In Ladybug this is a raw pointer `overflow_value_t {numElements, value}`.
/// Here we model it with owned storage for safe Rust.
#[derive(Debug, Clone)]
pub struct OverflowValue {
    pub num_elements: u64,
    pub values: Vec<Value>,
}

impl OverflowValue {
    pub fn new(values: Vec<Value>) -> Self {
        Self {
            num_elements: values.len() as u64,
            values,
        }
    }

    pub fn empty() -> Self {
        Self {
            num_elements: 0,
            values: Vec::new(),
        }
    }
}

/// Logical data block for flat tuples.
///
/// In the C++ implementation `DataBlock` wraps a `MemoryBuffer`.
/// Here it is a simple `Vec<u8>`-like container plus tuple count.
#[derive(Debug)]
pub struct DataBlock {
    pub num_tuples: u32,
    pub data: Vec<u8>,
    pub capacity: usize,
    pub num_bytes_per_tuple: u32,
}

impl DataBlock {
    pub fn new(capacity: usize) -> Self {
        Self::with_tuple_size(capacity, 0)
    }

    pub fn with_tuple_size(capacity: usize, num_bytes_per_tuple: u32) -> Self {
        Self {
            num_tuples: 0,
            data: vec![0u8; capacity],
            capacity,
            num_bytes_per_tuple,
        }
    }

    pub fn free_size(&self) -> usize {
        let used = self.num_tuples as usize * self.num_bytes_per_tuple as usize;
        self.capacity.saturating_sub(used)
    }

    pub fn used_bytes(&self) -> usize {
        self.capacity.saturating_sub(self.free_size())
    }

    pub fn can_hold(&self, tuples: u32) -> bool {
        (self.num_tuples + tuples) as usize * self.num_bytes_per_tuple as usize <= self.capacity
    }
}

/// Collection of data blocks.
#[derive(Debug, Default)]
pub struct DataBlockCollection {
    pub blocks: Vec<DataBlock>,
    pub num_bytes_per_tuple: u32,
    pub num_tuples_per_block: u32,
}

impl DataBlockCollection {
    pub fn new(num_bytes_per_tuple: u32, num_tuples_per_block: u32) -> Self {
        Self {
            blocks: Vec::new(),
            num_bytes_per_tuple,
            num_tuples_per_block,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn need_allocation(&self, size: u64) -> bool {
        if self.is_empty() {
            return true;
        }
        if size == 0 {
            return false;
        }
        if let Some(last) = self.blocks.last() {
            (last.free_size() as u64) < size
        } else {
            true
        }
    }

    pub fn allocate_block(&mut self) -> &mut DataBlock {
        let capacity = self.num_bytes_per_tuple as usize * self.num_tuples_per_block as usize;
        let block = DataBlock::with_tuple_size(capacity, self.num_bytes_per_tuple);
        self.blocks.push(block);
        self.blocks.last_mut().expect("just pushed")
    }

    pub fn append_block(&mut self, block: DataBlock) {
        self.blocks.push(block);
    }

    pub fn merge(&mut self, mut other: DataBlockCollection) {
        self.blocks.append(&mut other.blocks);
    }

    pub fn total_tuples(&self) -> u64 {
        self.blocks.iter().map(|b| b.num_tuples as u64).sum()
    }
}

/// In-memory overflow buffer for variable-length data (strings etc.).
#[derive(Debug, Default)]
pub struct InMemOverflowBuffer {
    pub buffers: Vec<Vec<u8>>,
}

impl InMemOverflowBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocate(&mut self, size: usize) -> &mut [u8] {
        self.buffers.push(vec![0u8; size]);
        self.buffers.last_mut().expect("just pushed").as_mut_slice()
    }

    pub fn merge(&mut self, mut other: InMemOverflowBuffer) {
        self.buffers.append(&mut other.buffers);
    }

    pub fn reset(&mut self) {
        self.buffers.clear();
    }
}

/// FactorizedTable with flat/unflat hybrid storage.
///
/// Layout inspired by `ref/ladybug/src/processor/result/factorized_table.cpp`:
/// - `flat_data_blocks` holds one row per factorized tuple, flat columns inline,
///   unflat columns as `OverflowValue` (pointer + count).
/// - `unflat_overflow_blocks` holds the actual arrays for unflat columns.
/// - `in_mem_overflow_buffer` holds variable-length overflow (strings).
///
/// This Rust implementation simplifies raw byte handling by storing
/// `Value` vectors directly, while preserving the public API shape.
#[derive(Debug)]
pub struct FactorizedTable {
    pub schema: FactorizedTableSchema,
    pub num_tuples: u64,
    /// Flat column data: per tuple, per column value (for flat columns only;
    /// unflat positions hold a placeholder `Null` which tracks the overflow pointer).
    pub flat_tuples: Vec<Vec<Value>>,
    /// Overflow data: per tuple, per unflat column -> Vec<Value>.
    pub overflow_tuples: Vec<HashMap<usize, OverflowValue>>,
    /// Null maps per tuple.
    pub null_maps: Vec<Vec<bool>>,
    pub flat_data_blocks: DataBlockCollection,
    pub unflat_overflow_blocks: DataBlockCollection,
    pub in_mem_overflow_buffer: InMemOverflowBuffer,
}

impl FactorizedTable {
    pub fn new(schema: FactorizedTableSchema) -> Self {
        let flat_blocks = if schema.is_empty() {
            DataBlockCollection::default()
        } else {
            DataBlockCollection::new(schema.num_bytes_per_tuple, 2048)
        };
        Self {
            schema,
            num_tuples: 0,
            flat_tuples: Vec::new(),
            overflow_tuples: Vec::new(),
            null_maps: Vec::new(),
            flat_data_blocks: flat_blocks,
            unflat_overflow_blocks: DataBlockCollection::default(),
            in_mem_overflow_buffer: InMemOverflowBuffer::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(FactorizedTableSchema::empty())
    }

    pub fn is_empty(&self) -> bool {
        self.num_tuples == 0
    }

    pub fn num_tuples(&self) -> u64 {
        self.num_tuples
    }

    pub fn table_schema(&self) -> &FactorizedTableSchema {
        &self.schema
    }

    pub fn has_unflat_col(&self) -> bool {
        self.schema.columns.iter().any(|c| c.is_unflat)
    }

    pub fn has_no_null_guarantee(&self, col_idx: usize) -> bool {
        self.schema
            .get_column(col_idx)
            .map(|c| c.has_no_null_guarantee())
            .unwrap_or(true)
    }

    /// Append one factorized row.
    ///
    /// `vectors[i]` corresponds to column `i`:
    /// - flat column: exactly one value (or empty for null)
    /// - unflat column: zero or more values (may be empty for zero neighbors)
    pub fn append(&mut self, vectors: &[Vec<Value>]) -> Result<(), String> {
        if vectors.len() != self.schema.num_columns() {
            return Err(format!(
                "append: expected {} columns, got {}",
                self.schema.num_columns(),
                vectors.len()
            ));
        }
        let mut flat_row = Vec::with_capacity(vectors.len());
        let mut overflow_map = HashMap::new();
        let mut null_map = vec![false; vectors.len()];

        let mut may_contain_nulls_updates: Vec<Option<bool>> =
            vec![None; self.schema.num_columns()];
        for (col_idx, col_schema) in self.schema.columns.iter().enumerate() {
            let col_values = &vectors[col_idx];
            if col_schema.is_flat() {
                if col_values.is_empty() {
                    flat_row.push(Value::Null(graphdb_core::value::NullType::Null));
                    null_map[col_idx] = true;
                    may_contain_nulls_updates[col_idx] = Some(true);
                } else {
                    let val = col_values[0].clone();
                    if matches!(val, Value::Null(_)) {
                        null_map[col_idx] = true;
                        may_contain_nulls_updates[col_idx] = Some(true);
                    }
                    flat_row.push(val);
                }
            } else {
                let ov = OverflowValue::new(col_values.clone());
                for v in col_values {
                    if matches!(v, Value::Null(_)) {
                        null_map[col_idx] = true;
                        may_contain_nulls_updates[col_idx] = Some(true);
                        break;
                    }
                }
                flat_row.push(Value::Null(graphdb_core::value::NullType::Null));
                overflow_map.insert(col_idx, ov);
            }
        }
        for (idx, upd) in may_contain_nulls_updates.into_iter().enumerate() {
            if upd.is_some() {
                self.schema.columns[idx].may_contain_nulls = true;
            }
        }

        self.flat_tuples.push(flat_row);
        self.overflow_tuples.push(overflow_map);
        self.null_maps.push(null_map);
        self.num_tuples += 1;
        Ok(())
    }

    /// Append an empty tuple and return its index.
    pub fn append_empty_tuple(&mut self) -> usize {
        let num_cols = self.schema.num_columns();
        let flat_row = vec![Value::Null(graphdb_core::value::NullType::Null); num_cols];
        let overflow_map: HashMap<usize, OverflowValue> = HashMap::new();
        let null_map = vec![false; num_cols];
        self.flat_tuples.push(flat_row);
        self.overflow_tuples.push(overflow_map);
        self.null_maps.push(null_map);
        let idx = self.num_tuples as usize;
        self.num_tuples += 1;
        idx
    }

    /// Scan `num_tuples_to_scan` rows starting at `tuple_idx` into `vectors`.
    ///
    /// `vectors[i]` will be filled with the column's values. For flat columns
    /// one value per scanned tuple, for unflat columns the overflow length
    /// (must scan single tuple when unflat present).
    pub fn scan(
        &self,
        vectors: &mut [Vec<Value>],
        start: usize,
        count: usize,
    ) -> Result<(), String> {
        if start + count > self.num_tuples as usize {
            return Err("scan out of bounds".to_string());
        }
        if vectors.len() != self.schema.num_columns() {
            return Err("scan: vectors len mismatch schema".to_string());
        }
        for v in vectors.iter_mut() {
            v.clear();
        }
        for row_idx in start..start + count {
            let flat_row = &self.flat_tuples[row_idx];
            let overflow = &self.overflow_tuples[row_idx];
            for (col_idx, col) in self.schema.columns.iter().enumerate() {
                if col.is_flat() {
                    vectors[col_idx].push(flat_row[col_idx].clone());
                } else {
                    // Unflat: caller must handle overflow separately.
                    // For bulk scan we extend with overflow values but
                    // contract says numTuplesToRead must be 1 if unflat.
                    if count != 1 {
                        return Err("cannot scan multiple tuples with unflat columns".to_string());
                    }
                    if let Some(ov) = overflow.get(&col_idx) {
                        vectors[col_idx].extend(ov.values.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Lookup helper for factorized probing (single tuple + selection).
    pub fn lookup_single(
        &self,
        col_idx: usize,
        tuple_idx: usize,
        out: &mut Vec<Value>,
    ) -> Result<(), String> {
        if tuple_idx >= self.num_tuples as usize {
            return Err("lookup out of bounds".to_string());
        }
        let col = self
            .schema
            .get_column(col_idx)
            .ok_or_else(|| "col_idx out of range".to_string())?;
        if col.is_flat() {
            out.push(self.flat_tuples[tuple_idx][col_idx].clone());
        } else {
            let ov = self.overflow_tuples[tuple_idx]
                .get(&col_idx)
                .ok_or_else(|| "overflow missing".to_string())?;
            out.extend(ov.values.clone());
        }
        Ok(())
    }

    /// Merge another table into this one.
    pub fn merge(&mut self, mut other: FactorizedTable) -> Result<(), String> {
        if self.schema != other.schema {
            return Err("merge: schemas differ".to_string());
        }
        if other.num_tuples == 0 {
            return Ok(());
        }
        // Propagate nullability
        for i in 0..self.schema.num_columns() {
            if !other.schema.columns[i].has_no_null_guarantee() {
                self.schema.columns[i].may_contain_nulls = true;
            }
        }
        self.flat_tuples.append(&mut other.flat_tuples);
        self.overflow_tuples.append(&mut other.overflow_tuples);
        self.null_maps.append(&mut other.null_maps);
        self.num_tuples += other.num_tuples;
        self.flat_data_blocks.merge(other.flat_data_blocks);
        self.unflat_overflow_blocks
            .merge(other.unflat_overflow_blocks);
        self.in_mem_overflow_buffer
            .merge(other.in_mem_overflow_buffer);
        Ok(())
    }

    /// Compute flat tuple count for a single factorized row.
    pub fn num_flat_tuples_for_row(&self, row_idx: usize) -> u64 {
        if row_idx >= self.num_tuples as usize {
            return 0;
        }
        let mut seen_groups: HashMap<FGroupPos, u64> = HashMap::new();
        for (col_idx, col) in self.schema.columns.iter().enumerate() {
            let gid = col.group_id;
            if seen_groups.contains_key(&gid) {
                continue;
            }
            let cnt = if col.is_flat() {
                1
            } else {
                self.overflow_tuples[row_idx]
                    .get(&col_idx)
                    .map(|ov| ov.num_elements)
                    .unwrap_or(1)
            };
            seen_groups.insert(gid, cnt);
        }
        seen_groups.values().product::<u64>().max(1)
    }

    pub fn total_num_flat_tuples(&self) -> u64 {
        (0..self.num_tuples as usize)
            .map(|i| self.num_flat_tuples_for_row(i))
            .sum()
    }

    pub fn clear(&mut self) {
        self.flat_tuples.clear();
        self.overflow_tuples.clear();
        self.null_maps.clear();
        self.num_tuples = 0;
        self.flat_data_blocks = DataBlockCollection::new(self.schema.num_bytes_per_tuple, 2048);
        self.unflat_overflow_blocks = DataBlockCollection::default();
        self.in_mem_overflow_buffer.reset();
    }

    pub fn get_tuple_flat_value(&self, tuple_idx: usize, col_idx: usize) -> Option<&Value> {
        self.flat_tuples.get(tuple_idx)?.get(col_idx)
    }

    pub fn get_tuple_overflow(&self, tuple_idx: usize, col_idx: usize) -> Option<&OverflowValue> {
        self.overflow_tuples.get(tuple_idx)?.get(&col_idx)
    }
}

fn row_layout_size(data_type_str: &str) -> u32 {
    match data_type_str.to_lowercase().as_str() {
        "int" | "bigint" | "int64" => 8,
        "double" | "float" => 8,
        "bool" => 1,
        "string" => 16,
        _ => 16,
    }
}

pub fn row_layout_size_for_value(value: &Value) -> u32 {
    match value {
        Value::Int(_) => 8,
        Value::BigInt(_) => 8,
        Value::Double(_) | Value::Float(_) => 8,
        Value::Bool(_) => 1,
        Value::String(_) => 16,
        Value::Null(_) => 1,
        _ => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

    fn make_schema() -> FactorizedTableSchema {
        // 2 columns: flat a.name (group 0), unflat b.name (group 1)
        let cols = vec![
            ColumnSchema::new(false, 0, 16),
            ColumnSchema::new(true, 1, 16),
        ];
        FactorizedTableSchema::new(cols)
    }

    #[test]
    fn schema_layout() {
        let schema = make_schema();
        assert_eq!(schema.num_columns(), 2);
        assert_eq!(schema.num_flat_columns(), 1);
        assert_eq!(schema.num_unflat_columns(), 1);
        assert!(schema.col_offsets.len() == 2);
    }

    #[test]
    fn append_and_scan_flat_unflat() {
        let schema = make_schema();
        let mut table = FactorizedTable::new(schema);
        // Row0: a= Alice, b=[Bob, Carl]
        table
            .append(&[
                vec![Value::string("Alice")],
                vec![Value::string("Bob"), Value::string("Carl")],
            ])
            .unwrap();
        // Row1: a= Dan, b=[Eve]
        table
            .append(&[vec![Value::string("Dan")], vec![Value::string("Eve")]])
            .unwrap();

        assert_eq!(table.num_tuples(), 2);
        assert_eq!(table.num_flat_tuples_for_row(0), 2);
        assert_eq!(table.num_flat_tuples_for_row(1), 1);
        assert_eq!(table.total_num_flat_tuples(), 3);
    }

    #[test]
    fn merge_tables() {
        let schema = make_schema();
        let mut t1 = FactorizedTable::new(schema.clone());
        t1.append(&[vec![Value::Int(1)], vec![Value::Int(10), Value::Int(20)]])
            .unwrap();

        let mut t2 = FactorizedTable::new(schema);
        t2.append(&[vec![Value::Int(2)], vec![Value::Int(30)]])
            .unwrap();

        t1.merge(t2).unwrap();
        assert_eq!(t1.num_tuples(), 2);
        assert_eq!(t1.total_num_flat_tuples(), 3);
    }

    #[test]
    fn scan_flat_only_batch() {
        let schema = FactorizedTableSchema::new(vec![
            ColumnSchema::new(false, 0, 8),
            ColumnSchema::new(false, 0, 8),
        ]);
        let mut table = FactorizedTable::new(schema);
        table
            .append(&[vec![Value::Int(1)], vec![Value::Int(100)]])
            .unwrap();
        table
            .append(&[vec![Value::Int(2)], vec![Value::Int(200)]])
            .unwrap();

        let mut vectors = vec![Vec::new(), Vec::new()];
        table.scan(&mut vectors, 0, 2).unwrap();
        assert_eq!(vectors[0], vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(vectors[1], vec![Value::Int(100), Value::Int(200)]);
    }

    #[test]
    fn overflow_value() {
        let ov = OverflowValue::new(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(ov.num_elements, 2);
        assert_eq!(ov.values.len(), 2);
    }

    #[test]
    fn clear_table() {
        let schema = make_schema();
        let mut table = FactorizedTable::new(schema);
        table
            .append(&[vec![Value::string("a")], vec![Value::string("b")]])
            .unwrap();
        assert_eq!(table.num_tuples(), 1);
        table.clear();
        assert_eq!(table.num_tuples(), 0);
        assert!(table.is_empty());
    }
}
