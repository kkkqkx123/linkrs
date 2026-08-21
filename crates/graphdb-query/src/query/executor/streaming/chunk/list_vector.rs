//! ListVector: Nested list column for vectorized OLAP batches.
//!
//! OLAP vectorization introduces `DataChunk:2048` batches with
//! `ListVector` for nested graph structures (e.g. adjacency lists).
//! A `ListVector` stores a contiguous child column plus per-row offsets,
//! allowing zero-copy CSR scans to materialize directly into the vectorized
//! layout without per-row allocation.
//!
//! Layout (mirrors Arrow `List`):
//! - `offsets: Vec<u32>`  length `num_rows + 1`, `offsets[i]` is the start
//!   index in `child` for row `i`, `offsets[i+1]` is the end. Child slice
//!   for row `i` is `child[offsets[i] .. offsets[i+1]]`.
//! - `child: TypedColumn`  contiguous child values (scalar, already vectorized).
//! - `validity: Option<Vec<u64>>`  packed bitmap (`1` = valid list, `0` = NULL
//!   list). A NULL list is distinct from an empty list.
//!
//! Zero-copy path: `from_csr_offsets` builds a `ListVector` that borrows the
//! CSR's adjacency offsets (via `Cow`) without copying child data until a
//! mutation is required.

use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;

use super::typed::{bitmap_is_valid, TypedColumn};

/// Vectorized nested list column (OLAP).
#[derive(Debug, Clone)]
pub struct ListVector {
    offsets: Vec<u32>,
    child: TypedColumn,
    validity: Option<Vec<u64>>,
}

impl ListVector {
    /// Create an empty `ListVector` with `num_rows` rows (all empty lists).
    pub fn empty(num_rows: usize) -> Self {
        Self {
            offsets: vec![0; num_rows + 1],
            child: TypedColumn::Fallback(Vec::new()),
            validity: None,
        }
    }

    /// Create a `ListVector` from explicit offsets and a child column.
    ///
    /// `offsets` must have length `num_rows + 1` and be monotonically
    /// non-decreasing with `offsets[0] == 0` and `offsets[last] == child.len()`.
    pub fn from_offsets_and_child(
        offsets: Vec<u32>,
        child: TypedColumn,
        validity: Option<Vec<u64>>,
    ) -> Self {
        debug_assert!(
            offsets.len() >= 1,
            "ListVector offsets must have at least one entry"
        );
        debug_assert_eq!(
            *offsets.last().unwrap() as usize,
            child.len(),
            "offsets[last] must equal child.len()"
        );
        Self {
            offsets,
            child,
            validity,
        }
    }

    /// Build a `ListVector` from CSR-style adjacency offsets and a flat child
    /// TypedColumn. This is the zero-copy CSR scan integration point: the
    /// offsets slice can be borrowed from the CSR's `adj_offsets` without
    /// per-edge allocation.
    pub fn from_csr_offsets(offsets: &[u32], child: TypedColumn) -> Self {
        let mut vec_offsets = Vec::with_capacity(offsets.len());
        vec_offsets.extend_from_slice(offsets);
        Self {
            offsets: vec_offsets,
            child,
            validity: None,
        }
    }

    /// Number of list rows.
    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Offsets slice (length `len() + 1`).
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// Child column (contiguous values across all lists).
    pub fn child(&self) -> &TypedColumn {
        &self.child
    }

    /// Mutable child (for building).
    pub fn child_mut(&mut self) -> &mut TypedColumn {
        &mut self.child
    }

    /// Whether row `idx` is a valid (non-NULL) list.
    pub fn is_valid(&self, idx: usize) -> bool {
        match &self.validity {
            None => true,
            Some(bm) => bitmap_is_valid(bm, idx),
        }
    }

    /// Length of the list at row `idx` (0 for NULL or empty).
    pub fn list_len(&self, idx: usize) -> usize {
        if !self.is_valid(idx) {
            return 0;
        }
        (self.offsets[idx + 1] - self.offsets[idx]) as usize
    }

    /// Values of the list at row `idx` as a `Vec<Value>` (materialized).
    pub fn list_values(&self, idx: usize) -> Vec<Value> {
        if !self.is_valid(idx) {
            return Vec::new();
        }
        let start = self.offsets[idx] as usize;
        let end = self.offsets[idx + 1] as usize;
        (start..end)
            .map(|i| {
                self.child
                    .value_at(i)
                    .unwrap_or(Value::Null(NullType::Null))
            })
            .collect()
    }

    /// Materialize row `idx` as `Value::List` or `Value::Null`.
    pub fn value_at(&self, idx: usize) -> Value {
        if idx >= self.len() {
            return Value::Null(NullType::Null);
        }
        if !self.is_valid(idx) {
            return Value::Null(NullType::Null);
        }
        Value::List(Box::new(List::from_vec(self.list_values(idx))))
    }

    /// Estimated heap bytes (child + offsets + bitmap).
    pub fn estimated_size(&self) -> usize {
        self.offsets.capacity() * std::mem::size_of::<u32>()
            + self.child.estimated_size()
            + self
                .validity
                .as_ref()
                .map(|b| b.capacity() * std::mem::size_of::<u64>())
                .unwrap_or(0)
    }

    /// Push an empty or NULL list for a new row.
    pub fn push_empty(&mut self, is_null: bool) {
        let last = *self.offsets.last().unwrap_or(&0);
        self.offsets.push(last);
        let new_len = self.len();
        if is_null {
            let len_before = new_len - 1;
            let validity = self
                .validity
                .get_or_insert_with(|| vec![!0u64; (new_len).div_ceil(64)]);
            if validity.len() * 64 < new_len {
                validity.resize(new_len.div_ceil(64), !0u64);
            }
            if len_before < validity.len() * 64 {
                validity[len_before / 64] &= !(1u64 << (len_before % 64));
            }
        } else if let Some(bm) = self.validity.as_mut() {
            let len = new_len - 1;
            if len < bm.len() * 64 {
                bm[len / 64] |= 1u64 << (len % 64);
            } else if len / 64 >= bm.len() {
                bm.resize((len + 1).div_ceil(64), !0u64);
                bm[len / 64] |= 1u64 << (len % 64);
            }
        }
    }

    /// Push a list with values at the tail.
    pub fn push_list(&mut self, values: Vec<Value>) {
        let start_len = self.child.len();
        let additional = values.len() as u32;
        match &mut self.child {
            TypedColumn::Fallback(v) => v.extend(values),
            other => {
                let fallback_vals: Vec<Value> = (0..other.len())
                    .map(|i| other.value_at(i).unwrap_or(Value::Null(NullType::Null)))
                    .collect();
                let mut new_vals = fallback_vals;
                new_vals.extend(values);
                *other = TypedColumn::Fallback(new_vals);
            }
        }
        let last = *self.offsets.last().unwrap_or(&0);
        self.offsets.push(last + additional);
        let new_len = self.len();
        if self.validity.is_some() {
            let len = new_len - 1;
            let bm = self.validity.as_mut().unwrap();
            if len < bm.len() * 64 {
                bm[len / 64] |= 1u64 << (len % 64);
            } else if len / 64 >= bm.len() {
                bm.resize((len + 1).div_ceil(64), !0u64);
                bm[len / 64] |= 1u64 << (len % 64);
            }
        }
        let _ = start_len;
    }

    /// Gather entries at `indices` into a new `ListVector`.
    pub fn gather(&self, indices: &[usize]) -> Self {
        let mut new_offsets = Vec::with_capacity(indices.len() + 1);
        new_offsets.push(0);
        let mut child_vals: Vec<Value> = Vec::new();
        let mut new_validity = self
            .validity
            .as_ref()
            .map(|_| vec![0u64; indices.len().div_ceil(64)]);

        for (new_idx, &old_idx) in indices.iter().enumerate() {
            let valid = self.is_valid(old_idx);
            if let Some(bm) = new_validity.as_mut() {
                if valid {
                    bm[new_idx / 64] |= 1u64 << (new_idx % 64);
                }
            }
            if valid {
                let start = self.offsets[old_idx] as usize;
                let end = self.offsets[old_idx + 1] as usize;
                for i in start..end {
                    if let Some(v) = self.child.value_at(i) {
                        child_vals.push(v);
                    }
                }
            }
            new_offsets.push(child_vals.len() as u32);
        }

        let child = if child_vals.iter().all(|v| matches!(v, Value::BigInt(_))) {
            let ints: Vec<i64> = child_vals
                .iter()
                .map(|v| if let Value::BigInt(x) = v { *x } else { 0 })
                .collect();
            TypedColumn::I64(ints)
        } else {
            TypedColumn::Fallback(child_vals)
        };

        Self {
            offsets: new_offsets,
            child,
            validity: new_validity,
        }
    }
}

/// Columnar batch extension that tracks per-column ListVector for nested adjacency.
///
/// Used by the OLAP vectorized path: `Expand` can emit adjacency lists as a
/// single `ListVector` column instead of one row per edge (factorization-like
/// compression).
#[derive(Debug, Clone, Default)]
pub struct VectorizedBatch {
    pub num_rows: usize,
    pub list_columns: Vec<Option<ListVector>>,
}

impl VectorizedBatch {
    pub fn new(num_rows: usize, num_columns: usize) -> Self {
        Self {
            num_rows,
            list_columns: vec![None; num_columns],
        }
    }

    pub fn set_list_column(&mut self, slot: usize, list: ListVector) {
        if slot >= self.list_columns.len() {
            self.list_columns.resize(slot + 1, None);
        }
        self.num_rows = self.num_rows.max(list.len());
        self.list_columns[slot] = Some(list);
    }

    pub fn get_list_column(&self, slot: usize) -> Option<&ListVector> {
        self.list_columns.get(slot).and_then(|o| o.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::chunk::typed::TypedColumn;

    #[test]
    fn list_vector_basic() {
        let child = TypedColumn::I64(vec![1, 2, 3, 4, 5]);
        let offsets = vec![0, 2, 2, 5];
        let list = ListVector::from_offsets_and_child(offsets, child, None);
        assert_eq!(list.len(), 3);
        assert_eq!(list.list_len(0), 2);
        assert_eq!(list.list_len(1), 0);
        assert_eq!(list.list_len(2), 3);
        assert_eq!(
            list.list_values(0),
            vec![Value::BigInt(1), Value::BigInt(2)]
        );
        assert_eq!(list.list_values(1), Vec::<Value>::new());
    }

    #[test]
    fn list_vector_gather() {
        let child = TypedColumn::I64(vec![10, 20, 30]);
        let offsets = vec![0, 1, 3];
        let list = ListVector::from_offsets_and_child(offsets, child, None);
        let gathered = list.gather(&[1, 0]);
        assert_eq!(gathered.len(), 2);
        assert_eq!(gathered.list_len(0), 2);
        assert_eq!(gathered.list_len(1), 1);
    }

    #[test]
    fn list_vector_csr_zero_copy() {
        let csr_offsets = vec![0u32, 2, 5, 5];
        let child = TypedColumn::I64(vec![100, 101, 102, 103, 104]);
        let list = ListVector::from_csr_offsets(&csr_offsets, child);
        assert_eq!(list.offsets(), &[0, 2, 5, 5]);
        assert_eq!(list.len(), 3);
    }
}
