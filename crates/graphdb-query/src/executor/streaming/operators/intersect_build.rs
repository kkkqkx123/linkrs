//! Sorted build-side tables for WCO intersect execution.
//!
//! Each build side maps a bound-node value to the adjacency rows sharing
//! it. [`IntersectBuild::finish`] sorts every adjacency list by the
//! intersect column once, so each probe turns into linear merge-style
//! intersections instead of repeated hashing.

use std::collections::HashMap;

use graphdb_core::Value;

/// Totally ordered sort key extracted from a [`Value`].
///
/// Integer node ids compare numerically; strings compare lexicographically;
/// every other value falls back to its debug rendering. The fallback is
/// deterministic within a run, which is all the merge intersection needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntersectKey {
    Int(i64),
    Str(String),
    Other(String),
}

impl From<&Value> for IntersectKey {
    fn from(value: &Value) -> Self {
        match value {
            Value::SmallInt(v) => Self::Int(*v as i64),
            Value::Int(v) => Self::Int(*v as i64),
            Value::BigInt(v) => Self::Int(*v),
            Value::String(s) => Self::Str(s.to_string()),
            Value::FixedString(s) => Self::Str(s.clone()),
            Value::Bool(v) => Self::Other(format!("bool:{v}")),
            other => Self::Other(format!("{other:?}")),
        }
    }
}

/// One build side of a WCO intersect: bound value -> sorted adjacency rows.
#[derive(Debug, Default)]
pub struct IntersectBuild {
    bound_col: usize,
    intersect_col: usize,
    table: HashMap<IntersectKey, Vec<Vec<Value>>>,
    sealed: bool,
}

impl IntersectBuild {
    /// Create a build side reading the bound value at `bound_col` and the
    /// intersect value at `intersect_col` from each build row.
    pub fn new(bound_col: usize, intersect_col: usize) -> Self {
        Self {
            bound_col,
            intersect_col,
            table: HashMap::new(),
            sealed: false,
        }
    }

    pub fn bound_col(&self) -> usize {
        self.bound_col
    }

    pub fn intersect_col(&self) -> usize {
        self.intersect_col
    }

    /// Append one build row. Rows too narrow to hold both columns are
    /// skipped so malformed chunks cannot poison the table.
    pub fn append_row(&mut self, row: Vec<Value>) {
        let width = self.bound_col.max(self.intersect_col) + 1;
        if row.len() < width {
            return;
        }
        debug_assert!(!self.sealed, "appended after finish; resort needed");
        let key = IntersectKey::from(&row[self.bound_col]);
        self.table.entry(key).or_default().push(row);
        self.sealed = false;
    }

    /// Append many build rows.
    pub fn append_rows(&mut self, rows: &[Vec<Value>]) {
        for row in rows {
            self.append_row(row.clone());
        }
    }

    /// Sort every adjacency list by the intersect column. Must be called
    /// after the last append and before the first probe.
    pub fn finish(&mut self) {
        for rows in self.table.values_mut() {
            rows.sort_by(|a, b| {
                IntersectKey::from(&a[self.intersect_col])
                    .cmp(&IntersectKey::from(&b[self.intersect_col]))
            });
        }
        self.sealed = true;
    }

    /// Adjacency rows for one bound value, sorted by the intersect column
    /// after [`IntersectBuild::finish`].
    pub fn lookup(&self, bound: &Value) -> &[Vec<Value>] {
        let key = IntersectKey::from(bound);
        self.table.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Number of distinct bound values.
    pub fn num_keys(&self) -> usize {
        self.table.len()
    }

    /// Total buffered rows across all bound values.
    pub fn num_rows(&self) -> usize {
        self.table.values().map(Vec::len).sum()
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    pub fn clear(&mut self) {
        self.table.clear();
        self.sealed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> IntersectBuild {
        let mut b = IntersectBuild::new(0, 1);
        b.append_rows(&[
            vec![Value::Int(1), Value::Int(20)],
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(2), Value::Int(30)],
        ]);
        b.finish();
        b
    }

    #[test]
    fn finish_sorts_adjacency_by_intersect_column() {
        let b = build();
        let rows = b.lookup(&Value::Int(1));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], Value::Int(10));
        assert_eq!(rows[1][1], Value::Int(20));
    }

    #[test]
    fn missing_bound_yields_empty_slice() {
        let b = build();
        assert!(b.lookup(&Value::Int(99)).is_empty());
    }

    #[test]
    fn narrow_rows_are_skipped() {
        let mut b = IntersectBuild::new(0, 1);
        b.append_row(vec![Value::Int(1)]);
        b.append_row(vec![Value::Int(1), Value::Int(5)]);
        b.finish();
        assert_eq!(b.num_rows(), 1);
    }

    #[test]
    fn key_ordering_groups_integers_before_strings() {
        assert!(IntersectKey::Int(1) < IntersectKey::Int(2));
        assert!(IntersectKey::Str("a".to_string()) < IntersectKey::Str("b".to_string()));
    }
}
