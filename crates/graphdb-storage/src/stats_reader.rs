//! Column Statistics Snapshot Reader
//!
//! Exposes per-column storage statistics to the query optimizer as immutable
//! snapshots. Data sources are the storage-side zone maps and persisted
//! column stats, so reading a snapshot costs O(chunk count) rather than
//! O(row count) and never materializes rows.
//!
//! The trait methods are defaulted so test stubs and partial engines only
//! implement what they can serve; consumers must fall back conservatively
//! when a snapshot is unavailable.

use std::sync::Arc;

use graphdb_core::Value;

/// Immutable point-in-time statistics for one property column of a vertex
/// tag or an edge type.
#[derive(Debug, Clone, Default)]
pub struct ColumnStatsSnapshot {
    /// Live row estimate for the owning relation (tag / edge type).
    pub row_count: u64,
    /// Number of null values, when tracked by the storage layer.
    pub null_count: Option<u64>,
    /// Distinct value count as an *upper-bound estimate* (sum of per-shard /
    /// per-partition NDVs), only when every contributing table tracks it.
    pub distinct_count: Option<u64>,
    /// Global minimum over non-null values (zone-map bounds are conservative:
    /// they only widen after writes, never shrink).
    pub min_value: Option<Value>,
    /// Global maximum over non-null values.
    pub max_value: Option<Value>,
}

impl ColumnStatsSnapshot {
    /// Whether this snapshot carries any usable bound for selectivity
    /// estimation.
    pub fn has_envelope(&self) -> bool {
        self.min_value.is_some() || self.max_value.is_some()
    }

    /// Merge `other` into `self` so that `self` covers the union of both
    /// snapshots' rows:
    /// - row counts add up;
    /// - min/max bounds widen using the numeric-aware comparison shared with
    ///   pushed-predicate evaluation;
    /// - null counts sum over the tables that track them (missing tracking
    ///   is treated as no contribution);
    /// - distinct counts are kept only when *both* sides carry the estimate
    ///   and then summed. Per-table NDVs are not disjoint, so the result is
    ///   an upper bound; a partially-known aggregate could underestimate
    ///   true NDV, hence it degrades to `None`.
    pub fn absorb(&mut self, other: &ColumnStatsSnapshot) {
        self.row_count += other.row_count;
        if let Some(n) = other.null_count {
            *self.null_count.get_or_insert(0) += n;
        }
        match (self.distinct_count, other.distinct_count) {
            (Some(a), Some(b)) => self.distinct_count = Some(a.saturating_add(b)),
            _ => self.distinct_count = None,
        }
        merge_min(&mut self.min_value, other.min_value.clone());
        merge_max(&mut self.max_value, other.max_value.clone());
    }
}

/// Keep the smaller of the two optional bounds, using the same numeric-aware
/// comparison as pushed-predicate evaluation so merged envelopes stay
/// consistent with zone-map pruning semantics.
pub(crate) fn merge_min(cur: &mut Option<Value>, candidate: Option<Value>) {
    let Some(v) = candidate else { return };
    match cur {
        Some(c)
            if crate::vertex::column_store::compare_values(c, &v)
                != std::cmp::Ordering::Greater => {}
        _ => *cur = Some(v),
    }
}

/// Keep the larger of the two optional bounds; see [`merge_min`].
pub(crate) fn merge_max(cur: &mut Option<Value>, candidate: Option<Value>) {
    let Some(v) = candidate else { return };
    match cur {
        Some(c)
            if crate::vertex::column_store::compare_values(c, &v) != std::cmp::Ordering::Less => {}
        _ => *cur = Some(v),
    }
}

/// Reader capability for optimizer-facing column statistics snapshots.
///
/// `relation` addressing follows the catalog naming: a tag name for vertex
/// columns and an edge type name for edge columns.
pub trait ColumnStatsReader: Send + Sync {
    /// Snapshot of one property column of `tag` inside `space`.
    fn vertex_column_stats(
        &self,
        _space: &str,
        _tag: &str,
        _column: &str,
    ) -> Option<Arc<ColumnStatsSnapshot>> {
        None
    }

    /// Snapshot of one property column of `edge_type` inside `space`.
    fn edge_column_stats(
        &self,
        _space: &str,
        _edge_type: &str,
        _column: &str,
    ) -> Option<Arc<ColumnStatsSnapshot>> {
        None
    }

    /// Monotonic data-version stamp: changes whenever data has been written,
    /// independent of schema changes. Used by the optimizer's statistics
    /// cache to detect stale estimates after DML.
    ///
    /// Contract: `0` means "no writes observed / unknown" and is reserved;
    /// engines backed by real data must return values that strictly increase
    /// across DML (they may start at any value > 0). Consumers must treat a
    /// return value of `0` as "cacheability unknown" rather than as a valid
    /// epoch. Returns 0 when unknown.
    fn stats_epoch(&self) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(row_count: u64, min: Option<Value>, max: Option<Value>) -> ColumnStatsSnapshot {
        ColumnStatsSnapshot {
            row_count,
            null_count: None,
            distinct_count: None,
            min_value: min,
            max_value: max,
        }
    }

    #[test]
    fn absorb_widens_bounds_across_mixed_numeric_kinds() {
        let mut acc = snap(10, Some(Value::Int(1)), Some(Value::BigInt(100)));
        acc.null_count = Some(2);
        acc.distinct_count = Some(7);

        let mut other = snap(5, Some(Value::Double(-0.5)), Some(Value::SmallInt(50)));
        other.null_count = Some(1);
        other.distinct_count = Some(3);
        acc.absorb(&other);

        assert_eq!(acc.row_count, 15);
        assert_eq!(acc.null_count, Some(3));
        // NDV is only kept when both sides carry it; sum is an upper bound.
        assert_eq!(acc.distinct_count, Some(10));
        assert_eq!(acc.min_value, Some(Value::Double(-0.5)));
        assert_eq!(acc.max_value, Some(Value::BigInt(100)));
    }

    #[test]
    fn absorb_drops_partial_distinct_counts() {
        let mut acc = snap(4, None, None);
        acc.distinct_count = Some(9);
        acc.absorb(&snap(2, None, None));
        assert_eq!(acc.distinct_count, None);
    }

    #[test]
    fn absorb_keeps_null_tracking_when_only_one_side_tracks() {
        let mut acc = snap(1, None, None);
        acc.null_count = None;
        let mut other = snap(1, None, None);
        other.null_count = Some(1);
        acc.absorb(&other);
        assert_eq!(acc.null_count, Some(1));

        // And the reverse order keeps the tracked contribution too.
        let mut acc = snap(1, None, None);
        acc.null_count = Some(1);
        acc.absorb(&snap(1, None, None));
        assert_eq!(acc.null_count, Some(1));
    }

    #[test]
    fn merge_helpers_compare_cross_type_numerically() {
        let mut min = Some(Value::Int(-2));
        merge_min(&mut min, Some(Value::Double(-2.5)));
        assert_eq!(min, Some(Value::Double(-2.5)));

        let mut max = Some(Value::Float(1.5));
        merge_max(&mut max, Some(Value::SmallInt(2)));
        assert_eq!(max, Some(Value::SmallInt(2)));

        // Non-numeric kinds fall back to native Value ordering.
        let mut max = Some(Value::string("P99"));
        merge_max(&mut max, Some(Value::string("P100")));
        assert_eq!(max, Some(Value::string("P99")));
    }
}
