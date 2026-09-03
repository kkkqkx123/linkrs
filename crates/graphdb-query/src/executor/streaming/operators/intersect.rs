//! WCO intersect probe execution over sorted adjacency lists.
//!
//! For each probe row, every build side contributes the adjacency list of
//! its bound value; the probe emits one output row per intersect value
//! present in ALL lists, crossed with the matching payload rows. Sorted
//! adjacency plus linear merge intersection keeps the per-probe cost
//! proportional to the adjacency sizes rather than their product.
//!
//! Execution core: this executor and [`IntersectBuild`](super::intersect_build::IntersectBuild)
//! back the streaming [`WcoIntersectOperator`](super::wco_operator::WcoIntersectOperator),
//! which drains chunk inputs into the build tables and probes them per
//! probe row. Keep the row layout (`probe ++ [intersect] ++ builds`) in
//! sync with the operator's output assembly.

use std::collections::HashMap;

use graphdb_core::value::NullType;
use graphdb_core::Value;

use super::intersect_build::{IntersectBuild, IntersectKey};

/// Linear merge intersection of two sorted key slices.
///
/// Both inputs must be sorted; duplicates collapse to a single output key.
/// Runs in `O(left.len() + right.len())`.
pub fn two_way_sorted_intersect(
    left: &[IntersectKey],
    right: &[IntersectKey],
) -> Vec<IntersectKey> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                if out.last() != Some(&left[i]) {
                    out.push(left[i].clone());
                }
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Incrementally intersect N sorted key lists, smallest first.
///
/// Empty input or any empty list yields an empty result.
pub fn multiway_sorted_intersect(lists: &[Vec<IntersectKey>]) -> Vec<IntersectKey> {
    if lists.is_empty() || lists.iter().any(Vec::is_empty) {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..lists.len()).collect();
    order.sort_by_key(|i| lists[*i].len());
    let mut acc = lists[order[0]].clone();
    for idx in order.into_iter().skip(1) {
        acc = two_way_sorted_intersect(&acc, &lists[idx]);
        if acc.is_empty() {
            break;
        }
    }
    acc
}

/// Per-build-side probe wiring: which probe column holds the bound value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntersectDataInfo {
    pub bound_col: usize,
}

/// N-way intersect executor: one probe side plus N sorted build tables.
///
/// Output row layout: `probe_row ++ [intersect_value] ++ build_row_1 ++ ...
/// ++ build_row_N`, where each `build_row_i` is a full matching build row.
/// Callers resolve column positions through the slot layout before
/// constructing the executor.
#[derive(Debug, Default)]
pub struct WcoIntersectExecutor {
    builds: Vec<IntersectBuild>,
    bound_cols: Vec<usize>,
}

impl WcoIntersectExecutor {
    /// Create an executor over finished build tables. `bound_cols[i]` is
    /// the probe-side column holding the bound value for `builds[i]`.
    pub fn new(builds: Vec<IntersectBuild>, bound_cols: Vec<usize>) -> Self {
        assert_eq!(
            builds.len(),
            bound_cols.len(),
            "one probe bound column per build side"
        );
        assert!(
            !builds.is_empty(),
            "WCO intersect needs at least one build side"
        );
        Self { builds, bound_cols }
    }

    pub fn num_builds(&self) -> usize {
        self.builds.len()
    }

    /// Release the build tables. The streaming operator moves tables out
    /// per probe chunk and restores them afterwards so consecutive chunks
    /// reuse the same sealed state without cloning.
    pub fn into_builds(self) -> Vec<IntersectBuild> {
        self.builds
    }

    /// Probe with one row; returns zero or more output rows.
    pub fn probe_row(&self, probe_row: &[Value]) -> Vec<Vec<Value>> {
        // 1. Look up every adjacency list; any miss kills the row.
        let mut adjacencies: Vec<&[Vec<Value>]> = Vec::with_capacity(self.builds.len());
        for (build, bound_col) in self.builds.iter().zip(self.bound_cols.iter()) {
            let Some(bound) = probe_row.get(*bound_col) else {
                return Vec::new();
            };
            let rows = build.lookup(bound);
            if rows.is_empty() {
                return Vec::new();
            }
            adjacencies.push(rows);
        }

        // 2. Group each adjacency by intersect key (lists are pre-sorted,
        // so keys come out sorted) and remember a representative value.
        let mut key_lists: Vec<Vec<IntersectKey>> = Vec::with_capacity(adjacencies.len());
        let mut groups: Vec<HashMap<IntersectKey, Vec<usize>>> =
            Vec::with_capacity(adjacencies.len());
        let mut representatives: HashMap<IntersectKey, Value> = HashMap::new();
        for (build_idx, rows) in adjacencies.iter().enumerate() {
            let intersect_col = self.builds[build_idx].intersect_col();
            let mut group: HashMap<IntersectKey, Vec<usize>> = HashMap::new();
            let mut keys: Vec<IntersectKey> = Vec::new();
            for (row_idx, row) in rows.iter().enumerate() {
                let Some(value) = row.get(intersect_col) else {
                    continue;
                };
                let key = IntersectKey::from(value);
                if !group.contains_key(&key) {
                    keys.push(key.clone());
                    if build_idx == 0 {
                        representatives
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                }
                group.entry(key).or_default().push(row_idx);
            }
            if keys.is_empty() {
                return Vec::new();
            }
            key_lists.push(keys);
            groups.push(group);
        }

        // 3. Merge-intersect the key lists.
        let common = multiway_sorted_intersect(&key_lists);
        if common.is_empty() {
            return Vec::new();
        }

        // 4. Emit the cartesian product of matching payloads per key.
        let mut out = Vec::new();
        for key in &common {
            let rep = representatives
                .get(key)
                .cloned()
                .unwrap_or(Value::Null(NullType::Null));
            let base: Vec<Value> = probe_row
                .iter()
                .cloned()
                .chain(std::iter::once(rep))
                .collect();
            let mut partials = vec![base];
            for (build_idx, group) in groups.iter().enumerate() {
                let Some(matched) = group.get(key) else {
                    partials.clear();
                    break;
                };
                let rows = adjacencies[build_idx];
                let mut next = Vec::with_capacity(partials.len() * matched.len());
                for partial in &partials {
                    for row_idx in matched {
                        let mut row = partial.clone();
                        row.extend(rows[*row_idx].iter().cloned());
                        next.push(row);
                    }
                }
                partials = next;
            }
            out.extend(partials);
        }
        out
    }

    /// Probe with many rows, concatenating the per-row outputs.
    pub fn probe_rows(&self, probe_rows: &[Vec<Value>]) -> Vec<Vec<Value>> {
        let mut out = Vec::new();
        for row in probe_rows {
            out.extend(self.probe_row(row));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(v: i32) -> IntersectKey {
        IntersectKey::Int(v as i64)
    }

    #[test]
    fn two_way_merge_basics() {
        assert_eq!(
            two_way_sorted_intersect(&[k(1), k(2), k(3)], &[k(2), k(3), k(4)]),
            vec![k(2), k(3)]
        );
        assert!(two_way_sorted_intersect(&[k(1)], &[k(2)]).is_empty());
        assert!(two_way_sorted_intersect(&[], &[k(1)]).is_empty());
    }

    #[test]
    fn two_way_merge_collapses_duplicates() {
        assert_eq!(
            two_way_sorted_intersect(&[k(1), k(1), k(2)], &[k(1), k(2), k(2)]),
            vec![k(1), k(2)]
        );
    }

    #[test]
    fn multiway_smallest_first() {
        let lists = vec![
            vec![k(1), k(2), k(3), k(4)],
            vec![k(2), k(4)],
            vec![k(1), k(2), k(4)],
        ];
        assert_eq!(multiway_sorted_intersect(&lists), vec![k(2), k(4)]);
        assert!(multiway_sorted_intersect(&[vec![k(1)], vec![]]).is_empty());
        assert!(multiway_sorted_intersect(&[]).is_empty());
    }

    fn triangle_executor() -> WcoIntersectExecutor {
        // Probe (a, b); build1 keyed by a carries c; build2 keyed by b carries c.
        let mut b1 = IntersectBuild::new(0, 1);
        b1.append_rows(&[
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(1), Value::Int(20)],
            vec![Value::Int(2), Value::Int(30)],
        ]);
        b1.finish();
        let mut b2 = IntersectBuild::new(0, 1);
        b2.append_rows(&[
            vec![Value::Int(2), Value::Int(20)],
            vec![Value::Int(2), Value::Int(40)],
        ]);
        b2.finish();
        WcoIntersectExecutor::new(vec![b1, b2], vec![0, 1])
    }

    #[test]
    fn triangle_probe_emits_common_adjacency() {
        let exec = triangle_executor();
        let out = exec.probe_row(&[Value::Int(1), Value::Int(2)]);
        // Common c = {20}: probe ++ [c] ++ build1 row ++ build2 row.
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(20),
                Value::Int(1),
                Value::Int(20),
                Value::Int(2),
                Value::Int(20),
            ]
        );
    }

    #[test]
    fn probe_without_common_adjacency_emits_nothing() {
        let exec = triangle_executor();
        // a=2 -> {30}; b=2 -> {20, 40}: disjoint.
        assert!(exec.probe_row(&[Value::Int(2), Value::Int(2)]).is_empty());
        // Unknown bound value.
        assert!(exec.probe_row(&[Value::Int(9), Value::Int(2)]).is_empty());
    }

    #[test]
    fn duplicate_adjacency_fans_out() {
        let mut b1 = IntersectBuild::new(0, 1);
        b1.append_rows(&[
            vec![Value::Int(1), Value::Int(7), Value::string("x")],
            vec![Value::Int(1), Value::Int(7), Value::string("y")],
        ]);
        b1.finish();
        let mut b2 = IntersectBuild::new(0, 1);
        b2.append_rows(&[vec![Value::Int(1), Value::Int(7), Value::string("z")]]);
        b2.finish();
        let exec = WcoIntersectExecutor::new(vec![b1, b2], vec![0, 0]);
        let out = exec.probe_row(&[Value::Int(1)]);
        assert_eq!(out.len(), 2);
    }
}
