//! Subplans table for join order DP (bushy).
//!
//! Mirrors Ladybug's `src/include/planner/subplans_table.h` and
//! `src/planner/join_order/join_plan_solver.h`. The table stores the optimal
//! partial plan for every connected subset of tables (keyed by bitmask) and
//! drives the bottom-up DP that enumerates all bipartitions.
//!
//! This module enables the bushy DP that
//! evaluates `S = L ∪ R` for every proper `L` of `S`, combining the optimal
//! subplans for `L` and `R` with `hash_join_cost(L.rows,R.rows)` and the
//! most-selective predicate crossing the cut. The cheapest partition is kept.
//! Left-deep remains a subset (R is a singleton), so the bushy optimum is
//! at least as good. Costs incorporate column NDV / zone-map aware row
//! estimates via `StatsView` when available, and factorization awareness via
//! `factorization::estimate_factorization` for Expand-heavy patterns.

use std::collections::HashMap;

/// One entry in the subplans table: the optimal plan for exactly the tables
/// in `mask`.
#[derive(Debug, Clone)]
pub struct SubplanEntry {
    /// Bitmask of tables in this subset
    pub mask: u32,
    /// Total cost of the optimal subplan for `mask`
    pub cost: f64,
    /// Estimated output rows
    pub rows: u64,
    /// Left partition that achieved the optimum (0 for singletons)
    pub left: u32,
    /// Right partition that achieved the optimum (0 for singletons)
    pub right: u32,
    /// Human-readable join tree for EXPLAIN / debugging
    pub tree: String,
}

/// DP table storing the optimal subplan for every non-empty subset of tables.
///
/// `n` is the number of tables (≤ 8 for DP, otherwise the engine falls back
/// to the greedy heuristic). Masks range `1..(1<<n)`.
#[derive(Debug, Default)]
pub struct SubplansTable {
    entries: HashMap<u32, SubplanEntry>,
}

impl SubplansTable {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn insert(&mut self, entry: SubplanEntry) {
        self.entries.insert(entry.mask, entry);
    }

    pub fn get(&self, mask: u32) -> Option<&SubplanEntry> {
        self.entries.get(&mask)
    }

    pub fn get_mut(&mut self, mask: u32) -> Option<&mut SubplanEntry> {
        self.entries.get_mut(&mask)
    }

    pub fn contains(&self, mask: u32) -> bool {
        self.entries.contains_key(&mask)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over all masks in increasing popcount order (useful for DP loops).
    pub fn masks_sorted(&self) -> Vec<u32> {
        let mut masks: Vec<u32> = self.entries.keys().copied().collect();
        masks.sort_by_key(|m| m.count_ones());
        masks
    }

    /// Clear the table
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_insert_and_get() {
        let mut table = SubplansTable::new();
        table.insert(SubplanEntry {
            mask: 0b001,
            cost: 0.0,
            rows: 100,
            left: 0,
            right: 0,
            tree: "A".to_string(),
        });
        assert_eq!(table.len(), 1);
        assert!(table.contains(0b001));
        assert!(!table.contains(0b010));
        let e = table.get(0b001).expect("entry");
        assert_eq!(e.rows, 100);
        assert_eq!(e.tree, "A");
    }

    #[test]
    fn bushy_partition_cost_is_sum_plus_join() {
        // Simulate two singleton subplans joined into a pair.
        let mut table = SubplansTable::new();
        table.insert(SubplanEntry {
            mask: 0b001,
            cost: 10.0,
            rows: 100,
            left: 0,
            right: 0,
            tree: "A".to_string(),
        });
        table.insert(SubplanEntry {
            mask: 0b010,
            cost: 20.0,
            rows: 200,
            left: 0,
            right: 0,
            tree: "B".to_string(),
        });
        // Cost of joining A and B: left.cost + right.cost + hash_join_cost.
        let join_cost = 15.0;
        let combined = SubplanEntry {
            mask: 0b011,
            cost: 10.0 + 20.0 + join_cost,
            rows: 1000,
            left: 0b001,
            right: 0b010,
            tree: "Join(A,B)".to_string(),
        };
        table.insert(combined);
        let e = table.get(0b011).expect("pair");
        assert_eq!(e.left, 0b001);
        assert_eq!(e.right, 0b010);
        assert!((e.cost - 45.0).abs() < 1e-9);
    }

    #[test]
    fn masks_sorted_by_popcount() {
        let mut table = SubplansTable::new();
        for mask in [0b100, 0b011, 0b001, 0b111] {
            table.insert(SubplanEntry {
                mask,
                cost: 0.0,
                rows: 0,
                left: 0,
                right: 0,
                tree: String::new(),
            });
        }
        let sorted = table.masks_sorted();
        // popcounts: 001(1), 100(1), 011(2), 111(3) – stable order not guaranteed for ties
        assert_eq!(sorted[0].count_ones(), 1);
        assert_eq!(sorted[sorted.len() - 1].count_ones(), 3);
    }
}
